// AndroidToolsCam.dll — Media Foundation software camera source for MFCreateVirtualCamera.
// Loaded by the Windows Camera Frame Server; serves NV12 frames written by Android Tools
// into shared memory, or a placeholder when nothing is streaming.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <sddl.h>
#include <wrl.h>

#include <algorithm>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <thread>
#include <vector>

#include "shared.h"

using namespace Microsoft::WRL;

// Declared locally with the frame server's IID to avoid pulling in kernel-streaming headers.
MIDL_INTERFACE("28F54685-06FD-11D2-B27A-00A0C9223196")
IKsControl : public IUnknown {
    virtual HRESULT STDMETHODCALLTYPE KsProperty(PVOID, ULONG, PVOID, ULONG, ULONG*) = 0;
    virtual HRESULT STDMETHODCALLTYPE KsMethod(PVOID, ULONG, PVOID, ULONG, ULONG*) = 0;
    virtual HRESULT STDMETHODCALLTYPE KsEvent(PVOID, ULONG, PVOID, ULONG, ULONG*) = 0;
};

static const GUID PINNAME_VIDEO_CAPTURE_ = {0xfb6c4281, 0x0353, 0x11d1, {0x90, 0x5f, 0x00, 0x00, 0xc0, 0xcc, 0x16, 0xba}};
static const GUID KSCAMERAPROFILE_Legacy_ = {0xb4894d81, 0x62b7, 0x4eec, {0x87, 0x40, 0x80, 0x65, 0x8c, 0x4a, 0x9d, 0x3e}};

static HMODULE g_module = nullptr;

#define KS_NOT_FOUND HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND)

// ── frame source ───────────────────────────────────────────────────────

class FrameReader {
public:
    ~FrameReader() {
        if (view_) UnmapViewOfFile(view_);
        if (map_) CloseHandle(map_);
    }

    // Pull the newest frame from shared memory if one was written.
    void Poll() {
        if (!Ensure()) return;
        UINT64 c1 = view_->counter;
        if ((c1 & 1) || c1 == counter_ || view_->magic != AT_MAGIC) return;
        UINT32 w = view_->width, h = view_->height;
        if (!w || !h || w > AT_MAX_WIDTH || h > AT_MAX_HEIGHT || (w & 1) || (h & 1)) return;
        size_t size = (size_t)w * h * 3 / 2;
        scratch_.resize(size);
        memcpy(scratch_.data(), (BYTE*)view_ + AT_HEADER_SIZE, size);
        if (view_->counter != c1) return;  // torn read, keep previous frame
        frame_.swap(scratch_);
        width_ = w;
        height_ = h;
        counter_ = c1;
        updated_ = GetTickCount64();
    }

    void Render(BYTE* dst, UINT32 dw, UINT32 dh) {
        Poll();
        // The writer refreshes `tick` even when the picture is static.
        bool live = width_ && view_ && GetTickCount64() - std::max(view_->tick, updated_) < 2500;
        if (live) {
            Scale(dst, dw, dh);
        } else {
            Placeholder(dst, dw, dh);
        }
    }

private:
    bool Ensure() {
        if (view_) return true;
        ULONGLONG now = GetTickCount64();
        if (tried_ && now - tried_ < 1000) return false;
        tried_ = now;
        PSECURITY_DESCRIPTOR sd = nullptr;
        SECURITY_ATTRIBUTES sa = {sizeof(sa), nullptr, FALSE};
        if (ConvertStringSecurityDescriptorToSecurityDescriptorW(AT_MAP_SDDL, SDDL_REVISION_1, &sd, nullptr)) {
            sa.lpSecurityDescriptor = sd;
        }
        map_ = CreateFileMappingW(INVALID_HANDLE_VALUE, &sa, PAGE_READWRITE, 0, AT_MAP_SIZE, AT_MAP_NAME);
        if (sd) LocalFree(sd);
        if (!map_) map_ = OpenFileMappingW(FILE_MAP_READ, FALSE, AT_MAP_NAME);
        if (!map_) return false;
        view_ = (AtFrameHeader*)MapViewOfFile(map_, FILE_MAP_READ, 0, 0, AT_MAP_SIZE);
        if (!view_) {
            CloseHandle(map_);
            map_ = nullptr;
        }
        return view_ != nullptr;
    }

    void Scale(BYTE* dst, UINT32 dw, UINT32 dh) {
        const BYTE* src = frame_.data();
        UINT32 sw = width_, sh = height_;
        if (sw == dw && sh == dh) {
            memcpy(dst, src, (size_t)dw * dh * 3 / 2);
            return;
        }
        double s = std::min((double)dw / sw, (double)dh / sh);
        UINT32 tw = std::max<UINT32>(2, (UINT32)(sw * s) & ~1u);
        UINT32 th = std::max<UINT32>(2, (UINT32)(sh * s) & ~1u);
        UINT32 ox = ((dw - tw) / 2) & ~1u, oy = ((dh - th) / 2) & ~1u;
        Clear(dst, dw, dh);

        if (xmap_.size() != tw || mapped_sw_ != sw || mapped_tw_ != tw) {
            xmap_.resize(tw);
            for (UINT32 x = 0; x < tw; x++) xmap_[x] = (UINT32)((UINT64)x * sw / tw);
            mapped_sw_ = sw;
            mapped_tw_ = tw;
        }
        for (UINT32 y = 0; y < th; y++) {
            const BYTE* srow = src + (size_t)((UINT64)y * sh / th) * sw;
            BYTE* drow = dst + (size_t)(oy + y) * dw + ox;
            for (UINT32 x = 0; x < tw; x++) drow[x] = srow[xmap_[x]];
        }
        const BYTE* suv = src + (size_t)sw * sh;
        BYTE* duv = dst + (size_t)dw * dh;
        for (UINT32 y = 0; y < th / 2; y++) {
            UINT32 sy = std::min(sh / 2 - 1, (UINT32)((UINT64)y * sh / th));
            const BYTE* srow = suv + (size_t)sy * sw;
            BYTE* drow = duv + (size_t)(oy / 2 + y) * dw + ox;
            for (UINT32 x = 0; x < tw / 2; x++) {
                UINT32 sx = std::min(sw / 2 - 1, xmap_[x * 2] / 2);
                drow[x * 2] = srow[sx * 2];
                drow[x * 2 + 1] = srow[sx * 2 + 1];
            }
        }
    }

    static void Clear(BYTE* dst, UINT32 w, UINT32 h) {
        memset(dst, 16, (size_t)w * h);
        memset(dst + (size_t)w * h, 128, (size_t)w * h / 2);
    }

    static void Rect(BYTE* dst, UINT32 dw, int x0, int y0, int x1, int y1, BYTE luma) {
        for (int y = y0; y < y1; y++)
            memset(dst + (size_t)y * dw + x0, luma, (size_t)(x1 - x0));
    }

    // Black frame with a dim phone outline: "camera is present, nothing is streaming".
    static void Placeholder(BYTE* dst, UINT32 dw, UINT32 dh) {
        Clear(dst, dw, dh);
        int ph = (int)(dh * 0.28), pw = ph / 2, t = std::max(2, (int)dh / 200);
        int x0 = ((int)dw - pw) / 2, y0 = ((int)dh - ph) / 2;
        const BYTE l = 58;
        Rect(dst, dw, x0, y0, x0 + pw, y0 + t, l);
        Rect(dst, dw, x0, y0 + ph - t, x0 + pw, y0 + ph, l);
        Rect(dst, dw, x0, y0, x0 + t, y0 + ph, l);
        Rect(dst, dw, x0 + pw - t, y0, x0 + pw, y0 + ph, l);
        int bw = pw / 4;
        Rect(dst, dw, x0 + (pw - bw) / 2, y0 + ph - t * 5, x0 + (pw + bw) / 2, y0 + ph - t * 4, l);
    }

    HANDLE map_ = nullptr;
    AtFrameHeader* view_ = nullptr;
    ULONGLONG tried_ = 0;
    std::vector<BYTE> frame_, scratch_;
    std::vector<UINT32> xmap_;
    UINT32 width_ = 0, height_ = 0, mapped_sw_ = 0, mapped_tw_ = 0;
    UINT64 counter_ = 0;
    ULONGLONG updated_ = 0;
};

// ── stream ─────────────────────────────────────────────────────────────

class MediaSource;

class MediaStream
    : public RuntimeClass<RuntimeClassFlags<ClassicCom>,
                          ChainInterfaces<IMFMediaStream2, IMFMediaStream, IMFMediaEventGenerator>, IKsControl> {
public:
    HRESULT Initialize(IMFMediaSource* source, IMFStreamDescriptor* descriptor) {
        source_ = source;
        descriptor_ = descriptor;
        return MFCreateEventQueue(&queue_);
    }

    ~MediaStream() { StopWorker(); }

    // IMFMediaEventGenerator
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->GetEvent(flags, event) : MF_E_SHUTDOWN;
    }
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* cb, IUnknown* state) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->BeginGetEvent(cb, state) : MF_E_SHUTDOWN;
    }
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->EndGetEvent(result, event) : MF_E_SHUTDOWN;
    }
    STDMETHODIMP QueueEvent(MediaEventType type, REFGUID ext, HRESULT status, const PROPVARIANT* value) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->QueueEventParamVar(type, ext, status, value) : MF_E_SHUTDOWN;
    }

    // IMFMediaStream
    STDMETHODIMP GetMediaSource(IMFMediaSource** source) override {
        if (!source) return E_POINTER;
        std::lock_guard<std::mutex> g(lock_);
        if (!source_) return MF_E_SHUTDOWN;
        *source = source_;
        source_->AddRef();
        return S_OK;
    }
    STDMETHODIMP GetStreamDescriptor(IMFStreamDescriptor** descriptor) override {
        if (!descriptor) return E_POINTER;
        std::lock_guard<std::mutex> g(lock_);
        if (!descriptor_) return MF_E_SHUTDOWN;
        return descriptor_.CopyTo(descriptor);
    }
    STDMETHODIMP RequestSample(IUnknown* token) override {
        std::lock_guard<std::mutex> g(lock_);
        if (!queue_) return MF_E_SHUTDOWN;
        if (state_ != MF_STREAM_STATE_RUNNING) return MF_E_MEDIA_SOURCE_WRONGSTATE;
        pending_.push_back(ComPtr<IUnknown>(token));
        wake_.notify_one();
        return S_OK;
    }

    // IMFMediaStream2
    STDMETHODIMP SetStreamState(MF_STREAM_STATE value) override {
        if (value == MF_STREAM_STATE_RUNNING) return Start();
        if (value == MF_STREAM_STATE_STOPPED) return Stop();
        std::lock_guard<std::mutex> g(lock_);
        state_ = value;
        return S_OK;
    }
    STDMETHODIMP GetStreamState(MF_STREAM_STATE* value) override {
        if (!value) return E_POINTER;
        std::lock_guard<std::mutex> g(lock_);
        *value = state_;
        return S_OK;
    }

    // IKsControl
    STDMETHODIMP KsProperty(PVOID, ULONG, PVOID, ULONG, ULONG*) override { return KS_NOT_FOUND; }
    STDMETHODIMP KsMethod(PVOID, ULONG, PVOID, ULONG, ULONG*) override { return KS_NOT_FOUND; }
    STDMETHODIMP KsEvent(PVOID, ULONG, PVOID, ULONG, ULONG*) override { return KS_NOT_FOUND; }

    HRESULT Start() {
        {
            std::lock_guard<std::mutex> g(lock_);
            if (!queue_) return MF_E_SHUTDOWN;
            state_ = MF_STREAM_STATE_RUNNING;
            if (!worker_.joinable()) {
                quit_ = false;
                worker_ = std::thread(&MediaStream::Run, this);
            }
        }
        return QueueEvent(MEStreamStarted, GUID_NULL, S_OK, nullptr);
    }

    HRESULT Stop() {
        {
            std::lock_guard<std::mutex> g(lock_);
            if (!queue_) return MF_E_SHUTDOWN;
            state_ = MF_STREAM_STATE_STOPPED;
            pending_.clear();
        }
        return QueueEvent(MEStreamStopped, GUID_NULL, S_OK, nullptr);
    }

    void Shutdown() {
        StopWorker();
        std::lock_guard<std::mutex> g(lock_);
        if (queue_) queue_->Shutdown();
        queue_.Reset();
        descriptor_.Reset();
        source_ = nullptr;
        pending_.clear();
    }

private:
    ComPtr<IMFMediaEventQueue> Queue() {
        std::lock_guard<std::mutex> g(lock_);
        return queue_;
    }

    void StopWorker() {
        {
            std::lock_guard<std::mutex> g(lock_);
            quit_ = true;
            wake_.notify_all();
        }
        if (worker_.joinable() && worker_.get_id() != std::this_thread::get_id()) worker_.join();
    }

    void Run() {
        LONGLONG next = 0;
        for (;;) {
            ComPtr<IUnknown> token;
            {
                std::unique_lock<std::mutex> g(lock_);
                wake_.wait(g, [this] { return quit_ || !pending_.empty(); });
                if (quit_) return;
                token = pending_.front();
                pending_.pop_front();
            }
            UINT32 width = 1280, height = 720, num = 30, den = 1;
            CurrentFormat(&width, &height, &num, &den);
            LONGLONG interval = (LONGLONG)10000000 * den / std::max<UINT32>(1, num);

            // Pace delivery to the negotiated frame rate.
            LONGLONG now = MFGetSystemTime();
            if (next > now) {
                Sleep((DWORD)((next - now) / 10000));
                now = MFGetSystemTime();
            }
            next = std::max(now, next) + interval;

            ComPtr<IMFSample> sample;
            if (FAILED(MakeSample(width, height, now, interval, token.Get(), &sample))) continue;

            std::lock_guard<std::mutex> g(lock_);
            if (queue_ && state_ == MF_STREAM_STATE_RUNNING)
                queue_->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.Get());
        }
    }

    void CurrentFormat(UINT32* w, UINT32* h, UINT32* num, UINT32* den) {
        ComPtr<IMFStreamDescriptor> desc;
        {
            std::lock_guard<std::mutex> g(lock_);
            desc = descriptor_;
        }
        ComPtr<IMFMediaTypeHandler> handler;
        ComPtr<IMFMediaType> type;
        if (desc && SUCCEEDED(desc->GetMediaTypeHandler(&handler)) &&
            SUCCEEDED(handler->GetCurrentMediaType(&type))) {
            MFGetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, w, h);
            MFGetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, num, den);
        }
    }

    HRESULT MakeSample(UINT32 w, UINT32 h, LONGLONG time, LONGLONG duration, IUnknown* token, IMFSample** out) {
        DWORD size = w * h * 3 / 2;
        ComPtr<IMFSample> sample;
        ComPtr<IMFMediaBuffer> buffer;
        HRESULT hr = MFCreateSample(&sample);
        if (SUCCEEDED(hr)) hr = MFCreateMemoryBuffer(size, &buffer);
        if (FAILED(hr)) return hr;
        BYTE* data = nullptr;
        hr = buffer->Lock(&data, nullptr, nullptr);
        if (FAILED(hr)) return hr;
        reader_.Render(data, w, h);
        buffer->Unlock();
        buffer->SetCurrentLength(size);
        sample->AddBuffer(buffer.Get());
        sample->SetSampleTime(time);
        sample->SetSampleDuration(duration);
        if (token) sample->SetUnknown(MFSampleExtension_Token, token);
        *out = sample.Detach();
        return S_OK;
    }

    std::mutex lock_;
    std::condition_variable wake_;
    std::thread worker_;
    bool quit_ = false;
    std::deque<ComPtr<IUnknown>> pending_;
    ComPtr<IMFMediaEventQueue> queue_;
    ComPtr<IMFStreamDescriptor> descriptor_;
    IMFMediaSource* source_ = nullptr;  // weak: the source owns the stream
    MF_STREAM_STATE state_ = MF_STREAM_STATE_STOPPED;
    FrameReader reader_;
};

// ── source ─────────────────────────────────────────────────────────────

class MediaSource
    : public RuntimeClass<RuntimeClassFlags<ClassicCom>,
                          ChainInterfaces<IMFMediaSource2, IMFMediaSourceEx, IMFMediaSource, IMFMediaEventGenerator>,
                          IMFGetService, IKsControl> {
public:
    HRESULT Initialize(IMFAttributes* attributes) {
        HRESULT hr = MFCreateEventQueue(&queue_);
        if (SUCCEEDED(hr)) hr = MFCreateAttributes(&attributes_, 4);
        if (SUCCEEDED(hr) && attributes) attributes->CopyAllItems(attributes_.Get());
        if (FAILED(hr)) return hr;

        // Sensor profile so modern apps (Windows Camera) accept the device.
        ComPtr<IMFSensorProfileCollection> profiles;
        ComPtr<IMFSensorProfile> profile;
        if (SUCCEEDED(MFCreateSensorProfileCollection(&profiles)) &&
            SUCCEEDED(MFCreateSensorProfile(KSCAMERAPROFILE_Legacy_, 0, nullptr, &profile))) {
            profile->AddProfileFilter(0, L"((RES==;FRT<=60,1;SUT==))");
            profiles->AddProfile(profile.Get());
            attributes_->SetUnknown(MF_DEVICEMFT_SENSORPROFILE_COLLECTION, profiles.Get());
        }

        static const UINT32 sizes[][3] = {{1920, 1080, 30}, {1280, 720, 30}, {1280, 720, 60}, {960, 540, 30},
                                          {640, 360, 30}};
        std::vector<ComPtr<IMFMediaType>> types;
        for (auto& s : sizes) {
            ComPtr<IMFMediaType> t;
            hr = MFCreateMediaType(&t);
            if (FAILED(hr)) return hr;
            t->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video);
            t->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12);
            MFSetAttributeSize(t.Get(), MF_MT_FRAME_SIZE, s[0], s[1]);
            MFSetAttributeRatio(t.Get(), MF_MT_FRAME_RATE, s[2], 1);
            MFSetAttributeRatio(t.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1);
            t->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive);
            t->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE);
            t->SetUINT32(MF_MT_FIXED_SIZE_SAMPLES, TRUE);
            t->SetUINT32(MF_MT_DEFAULT_STRIDE, s[0]);
            t->SetUINT32(MF_MT_SAMPLE_SIZE, s[0] * s[1] * 3 / 2);
            t->SetUINT32(MF_MT_AVG_BITRATE, s[0] * s[1] * 12 * s[2]);
            types.push_back(t);
        }
        std::vector<IMFMediaType*> raw;
        for (auto& t : types) raw.push_back(t.Get());

        ComPtr<IMFStreamDescriptor> desc;
        hr = MFCreateStreamDescriptor(0, (DWORD)raw.size(), raw.data(), &desc);
        if (FAILED(hr)) return hr;
        ComPtr<IMFMediaTypeHandler> handler;
        desc->GetMediaTypeHandler(&handler);
        handler->SetCurrentMediaType(raw[0]);
        desc->SetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, PINNAME_VIDEO_CAPTURE_);
        desc->SetUINT32(MF_DEVICESTREAM_STREAM_ID, 0);
        desc->SetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, 1);
        desc->SetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color);

        stream_ = Make<MediaStream>();
        if (!stream_) return E_OUTOFMEMORY;
        hr = stream_->Initialize(this, desc.Get());
        if (FAILED(hr)) return hr;
        IMFStreamDescriptor* descs[] = {desc.Get()};
        hr = MFCreatePresentationDescriptor(1, descs, &presentation_);
        if (SUCCEEDED(hr)) hr = presentation_->SelectStream(0);
        descriptor_ = desc;
        return hr;
    }

    // IMFMediaEventGenerator
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->GetEvent(flags, event) : MF_E_SHUTDOWN;
    }
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* cb, IUnknown* state) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->BeginGetEvent(cb, state) : MF_E_SHUTDOWN;
    }
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->EndGetEvent(result, event) : MF_E_SHUTDOWN;
    }
    STDMETHODIMP QueueEvent(MediaEventType type, REFGUID ext, HRESULT status, const PROPVARIANT* value) override {
        ComPtr<IMFMediaEventQueue> q = Queue();
        return q ? q->QueueEventParamVar(type, ext, status, value) : MF_E_SHUTDOWN;
    }

    // IMFMediaSource
    STDMETHODIMP GetCharacteristics(DWORD* characteristics) override {
        if (!characteristics) return E_POINTER;
        *characteristics = MFMEDIASOURCE_IS_LIVE;
        return S_OK;
    }
    STDMETHODIMP CreatePresentationDescriptor(IMFPresentationDescriptor** pd) override {
        if (!pd) return E_POINTER;
        std::lock_guard<std::mutex> g(lock_);
        if (!presentation_) return MF_E_SHUTDOWN;
        return presentation_->Clone(pd);
    }
    STDMETHODIMP Start(IMFPresentationDescriptor* pd, const GUID* format, const PROPVARIANT* position) override {
        if (!pd || !position) return E_INVALIDARG;
        if (format && *format != GUID_NULL) return MF_E_UNSUPPORTED_TIME_FORMAT;
        ComPtr<MediaStream> stream;
        ComPtr<IMFMediaEventQueue> queue;
        {
            std::lock_guard<std::mutex> g(lock_);
            if (!queue_) return MF_E_SHUTDOWN;
            stream = stream_;
            queue = queue_;
        }
        BOOL selected = FALSE;
        DWORD count = 0;
        pd->GetStreamDescriptorCount(&count);
        for (DWORD i = 0; i < count; i++) {
            BOOL sel = FALSE;
            ComPtr<IMFStreamDescriptor> sd;
            DWORD id = 0;
            if (SUCCEEDED(pd->GetStreamDescriptorByIndex(i, &sel, &sd)) && SUCCEEDED(sd->GetStreamIdentifier(&id)) &&
                id == 0) {
                selected = sel;
                // Adopt the media type the caller negotiated on its copy of the descriptor.
                ComPtr<IMFMediaTypeHandler> theirs, ours;
                ComPtr<IMFMediaType> type;
                if (sel && SUCCEEDED(sd->GetMediaTypeHandler(&theirs)) &&
                    SUCCEEDED(theirs->GetCurrentMediaType(&type)) &&
                    SUCCEEDED(descriptor_->GetMediaTypeHandler(&ours))) {
                    ours->SetCurrentMediaType(type.Get());
                }
            }
        }

        PROPVARIANT time;
        PropVariantInit(&time);
        time.vt = VT_I8;
        time.hVal.QuadPart = MFGetSystemTime();
        HRESULT hr = S_OK;
        if (selected) {
            hr = queue->QueueEventParamUnk(started_ ? MEUpdatedStream : MENewStream, GUID_NULL, S_OK,
                                           static_cast<IMFMediaStream*>(stream.Get()));
            started_ = true;
        }
        if (SUCCEEDED(hr)) hr = queue->QueueEventParamVar(MESourceStarted, GUID_NULL, S_OK, &time);
        if (SUCCEEDED(hr)) hr = selected ? stream->Start() : stream->Stop();
        return hr;
    }
    STDMETHODIMP Stop() override {
        ComPtr<MediaStream> stream;
        ComPtr<IMFMediaEventQueue> queue;
        {
            std::lock_guard<std::mutex> g(lock_);
            if (!queue_) return MF_E_SHUTDOWN;
            stream = stream_;
            queue = queue_;
        }
        stream->Stop();
        PROPVARIANT time;
        PropVariantInit(&time);
        time.vt = VT_I8;
        time.hVal.QuadPart = MFGetSystemTime();
        return queue->QueueEventParamVar(MESourceStopped, GUID_NULL, S_OK, &time);
    }
    STDMETHODIMP Pause() override { return MF_E_INVALID_STATE_TRANSITION; }
    STDMETHODIMP Shutdown() override {
        ComPtr<MediaStream> stream;
        {
            std::lock_guard<std::mutex> g(lock_);
            if (!queue_) return MF_E_SHUTDOWN;
            queue_->Shutdown();
            queue_.Reset();
            stream = stream_;
            stream_.Reset();
            presentation_.Reset();
            descriptor_.Reset();
        }
        if (stream) stream->Shutdown();
        return S_OK;
    }

    // IMFMediaSourceEx
    STDMETHODIMP GetSourceAttributes(IMFAttributes** attributes) override {
        if (!attributes) return E_POINTER;
        return attributes_.CopyTo(attributes);
    }
    STDMETHODIMP GetStreamAttributes(DWORD id, IMFAttributes** attributes) override {
        if (!attributes) return E_POINTER;
        if (id != 0) return MF_E_INVALIDSTREAMNUMBER;
        std::lock_guard<std::mutex> g(lock_);
        if (!descriptor_) return MF_E_SHUTDOWN;
        return descriptor_.CopyTo(attributes);
    }
    STDMETHODIMP SetD3DManager(IUnknown*) override { return S_OK; }

    // IMFMediaSource2
    STDMETHODIMP SetMediaType(DWORD id, IMFMediaType* type) override {
        if (id != 0) return MF_E_INVALIDSTREAMNUMBER;
        if (!type) return E_POINTER;
        std::lock_guard<std::mutex> g(lock_);
        if (!descriptor_) return MF_E_SHUTDOWN;
        ComPtr<IMFMediaTypeHandler> handler;
        HRESULT hr = descriptor_->GetMediaTypeHandler(&handler);
        return SUCCEEDED(hr) ? handler->SetCurrentMediaType(type) : hr;
    }

    // IMFGetService
    STDMETHODIMP GetService(REFGUID, REFIID, LPVOID*) override { return MF_E_UNSUPPORTED_SERVICE; }

    // IKsControl
    STDMETHODIMP KsProperty(PVOID, ULONG, PVOID, ULONG, ULONG*) override { return KS_NOT_FOUND; }
    STDMETHODIMP KsMethod(PVOID, ULONG, PVOID, ULONG, ULONG*) override { return KS_NOT_FOUND; }
    STDMETHODIMP KsEvent(PVOID, ULONG, PVOID, ULONG, ULONG*) override { return KS_NOT_FOUND; }

private:
    ComPtr<IMFMediaEventQueue> Queue() {
        std::lock_guard<std::mutex> g(lock_);
        return queue_;
    }

    std::mutex lock_;
    ComPtr<IMFMediaEventQueue> queue_;
    ComPtr<IMFAttributes> attributes_;
    ComPtr<IMFPresentationDescriptor> presentation_;
    ComPtr<IMFStreamDescriptor> descriptor_;
    ComPtr<MediaStream> stream_;
    bool started_ = false;
};

// ── activator ──────────────────────────────────────────────────────────

class Activator : public RuntimeClass<RuntimeClassFlags<ClassicCom>, ChainInterfaces<IMFActivate, IMFAttributes>> {
public:
    Activator() { MFStartup(MF_VERSION, MFSTARTUP_LITE); }
    ~Activator() {
        source_.Reset();
        attributes_.Reset();
        MFShutdown();
    }
    HRESULT Initialize() { return MFCreateAttributes(&attributes_, 1); }

    STDMETHODIMP ActivateObject(REFIID riid, void** ppv) override {
        if (!ppv) return E_POINTER;
        std::lock_guard<std::mutex> g(lock_);
        if (!source_) {
            auto source = Make<MediaSource>();
            if (!source) return E_OUTOFMEMORY;
            HRESULT hr = source->Initialize(attributes_.Get());
            if (FAILED(hr)) return hr;
            source_ = source;
        }
        return source_.CopyTo(riid, ppv);
    }
    // The caller keeps using the source it activated; only drop our reference here.
    STDMETHODIMP ShutdownObject() override {
        std::lock_guard<std::mutex> g(lock_);
        source_.Reset();
        return S_OK;
    }
    STDMETHODIMP DetachObject() override {
        std::lock_guard<std::mutex> g(lock_);
        source_.Reset();
        return S_OK;
    }

    // IMFAttributes — forwarded to an owned attribute store.
    STDMETHODIMP GetItem(REFGUID k, PROPVARIANT* v) override { return attributes_->GetItem(k, v); }
    STDMETHODIMP GetItemType(REFGUID k, MF_ATTRIBUTE_TYPE* t) override { return attributes_->GetItemType(k, t); }
    STDMETHODIMP CompareItem(REFGUID k, REFPROPVARIANT v, BOOL* r) override { return attributes_->CompareItem(k, v, r); }
    STDMETHODIMP Compare(IMFAttributes* a, MF_ATTRIBUTES_MATCH_TYPE m, BOOL* r) override {
        return attributes_->Compare(a, m, r);
    }
    STDMETHODIMP GetUINT32(REFGUID k, UINT32* v) override { return attributes_->GetUINT32(k, v); }
    STDMETHODIMP GetUINT64(REFGUID k, UINT64* v) override { return attributes_->GetUINT64(k, v); }
    STDMETHODIMP GetDouble(REFGUID k, double* v) override { return attributes_->GetDouble(k, v); }
    STDMETHODIMP GetGUID(REFGUID k, GUID* v) override { return attributes_->GetGUID(k, v); }
    STDMETHODIMP GetStringLength(REFGUID k, UINT32* v) override { return attributes_->GetStringLength(k, v); }
    STDMETHODIMP GetString(REFGUID k, LPWSTR s, UINT32 n, UINT32* l) override { return attributes_->GetString(k, s, n, l); }
    STDMETHODIMP GetAllocatedString(REFGUID k, LPWSTR* s, UINT32* l) override {
        return attributes_->GetAllocatedString(k, s, l);
    }
    STDMETHODIMP GetBlobSize(REFGUID k, UINT32* v) override { return attributes_->GetBlobSize(k, v); }
    STDMETHODIMP GetBlob(REFGUID k, UINT8* b, UINT32 n, UINT32* l) override { return attributes_->GetBlob(k, b, n, l); }
    STDMETHODIMP GetAllocatedBlob(REFGUID k, UINT8** b, UINT32* l) override { return attributes_->GetAllocatedBlob(k, b, l); }
    STDMETHODIMP GetUnknown(REFGUID k, REFIID r, LPVOID* p) override { return attributes_->GetUnknown(k, r, p); }
    STDMETHODIMP SetItem(REFGUID k, REFPROPVARIANT v) override { return attributes_->SetItem(k, v); }
    STDMETHODIMP DeleteItem(REFGUID k) override { return attributes_->DeleteItem(k); }
    STDMETHODIMP DeleteAllItems() override { return attributes_->DeleteAllItems(); }
    STDMETHODIMP SetUINT32(REFGUID k, UINT32 v) override { return attributes_->SetUINT32(k, v); }
    STDMETHODIMP SetUINT64(REFGUID k, UINT64 v) override { return attributes_->SetUINT64(k, v); }
    STDMETHODIMP SetDouble(REFGUID k, double v) override { return attributes_->SetDouble(k, v); }
    STDMETHODIMP SetGUID(REFGUID k, REFGUID v) override { return attributes_->SetGUID(k, v); }
    STDMETHODIMP SetString(REFGUID k, LPCWSTR v) override { return attributes_->SetString(k, v); }
    STDMETHODIMP SetBlob(REFGUID k, const UINT8* b, UINT32 n) override { return attributes_->SetBlob(k, b, n); }
    STDMETHODIMP SetUnknown(REFGUID k, IUnknown* v) override { return attributes_->SetUnknown(k, v); }
    STDMETHODIMP LockStore() override { return attributes_->LockStore(); }
    STDMETHODIMP UnlockStore() override { return attributes_->UnlockStore(); }
    STDMETHODIMP GetCount(UINT32* n) override { return attributes_->GetCount(n); }
    STDMETHODIMP GetItemByIndex(UINT32 i, GUID* k, PROPVARIANT* v) override { return attributes_->GetItemByIndex(i, k, v); }
    STDMETHODIMP CopyAllItems(IMFAttributes* dest) override { return attributes_->CopyAllItems(dest); }

private:
    std::mutex lock_;
    ComPtr<IMFAttributes> attributes_;
    ComPtr<MediaSource> source_;
};

// ── COM plumbing ───────────────────────────────────────────────────────

class CamClassFactory : public RuntimeClass<RuntimeClassFlags<ClassicCom>, IClassFactory> {
public:
    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** ppv) override {
        if (!ppv) return E_POINTER;
        *ppv = nullptr;
        if (outer) return CLASS_E_NOAGGREGATION;
        auto activator = Make<Activator>();
        if (!activator) return E_OUTOFMEMORY;
        HRESULT hr = activator->Initialize();
        return SUCCEEDED(hr) ? activator.CopyTo(riid, ppv) : hr;
    }
    STDMETHODIMP LockServer(BOOL) override { return S_OK; }
};

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = module;
        DisableThreadLibraryCalls(module);
    }
    return TRUE;
}

STDAPI DllGetClassObject(REFCLSID clsid, REFIID riid, LPVOID* ppv) {
    if (!ppv) return E_POINTER;
    *ppv = nullptr;
    if (clsid != CLSID_AndroidToolsCam) return CLASS_E_CLASSNOTAVAILABLE;
    auto factory = Make<CamClassFactory>();
    return factory ? factory.CopyTo(riid, ppv) : E_OUTOFMEMORY;
}

// Stay loaded: the frame server may still hold worker threads created by this module.
STDAPI DllCanUnloadNow() { return S_FALSE; }

STDAPI DllRegisterServer() {
    wchar_t path[MAX_PATH];
    if (!GetModuleFileNameW(g_module, path, MAX_PATH)) return HRESULT_FROM_WIN32(GetLastError());
    std::wstring key = std::wstring(L"SOFTWARE\\Classes\\CLSID\\") + AT_CLSID_STR;
    HKEY h;
    LSTATUS s = RegCreateKeyExW(HKEY_LOCAL_MACHINE, (key + L"\\InprocServer32").c_str(), 0, nullptr, 0, KEY_WRITE,
                                nullptr, &h, nullptr);
    if (s != ERROR_SUCCESS) return HRESULT_FROM_WIN32(s);
    RegSetValueExW(h, nullptr, 0, REG_SZ, (BYTE*)path, (DWORD)((wcslen(path) + 1) * sizeof(wchar_t)));
    static const wchar_t model[] = L"Both";
    RegSetValueExW(h, L"ThreadingModel", 0, REG_SZ, (BYTE*)model, sizeof(model));
    RegCloseKey(h);
    if (RegCreateKeyExW(HKEY_LOCAL_MACHINE, key.c_str(), 0, nullptr, 0, KEY_WRITE, nullptr, &h, nullptr) ==
        ERROR_SUCCESS) {
        RegSetValueExW(h, nullptr, 0, REG_SZ, (BYTE*)AT_CAMERA_NAME, sizeof(AT_CAMERA_NAME));
        RegCloseKey(h);
    }
    return S_OK;
}

STDAPI DllUnregisterServer() {
    std::wstring key = std::wstring(L"SOFTWARE\\Classes\\CLSID\\") + AT_CLSID_STR;
    LSTATUS s = RegDeleteTreeW(HKEY_LOCAL_MACHINE, key.c_str());
    return (s == ERROR_SUCCESS || s == ERROR_FILE_NOT_FOUND) ? S_OK : HRESULT_FROM_WIN32(s);
}
