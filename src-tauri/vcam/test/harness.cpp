// Drives AndroidToolsCam.dll the way the Frame Server does, without registration or admin rights.
#define NOMINMAX
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mferror.h>
#include <wrl.h>
#include <iostream>
#include "../shared.h"
using Microsoft::WRL::ComPtr;

#define CHECK(hr, what) if (FAILED(hr)) { std::cout << "FAIL " << what << " 0x" << std::hex << (unsigned)hr << "\n"; return 1; } else std::cout << "ok   " << what << "\n";

int wmain(int argc, wchar_t** argv) {
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION);
    HMODULE dll = LoadLibraryW(argv[1]);
    if (!dll) { std::cout << "LoadLibrary failed\n"; return 1; }
    auto get = (HRESULT(*)(REFCLSID, REFIID, LPVOID*))GetProcAddress(dll, "DllGetClassObject");
    ComPtr<IClassFactory> factory;
    CHECK(get(CLSID_AndroidToolsCam, IID_IClassFactory, &factory), "DllGetClassObject");
    ComPtr<IMFActivate> activate;
    CHECK(factory->CreateInstance(nullptr, IID_IMFActivate, &activate), "CreateInstance(IMFActivate)");
    ComPtr<IMFMediaSource> source;
    CHECK(activate->ActivateObject(IID_IMFMediaSource, &source), "ActivateObject");
    CHECK(activate->ShutdownObject(), "ShutdownObject (source must survive)");

    ComPtr<IMFMediaSourceEx> ex;
    CHECK(source.As(&ex), "QI IMFMediaSourceEx");
    ComPtr<IMFMediaSource2> s2;
    CHECK(source.As(&s2), "QI IMFMediaSource2");
    ComPtr<IMFAttributes> sa;
    CHECK(ex->GetStreamAttributes(0, &sa), "GetStreamAttributes");
    ComPtr<IUnknown> ks;
    CHECK(source->QueryInterface(IID_PPV_ARGS(&ks)), "QI IUnknown");
    static const IID IID_KS = {0x28F54685, 0x06FD, 0x11D2, {0xB2, 0x7A, 0x00, 0xA0, 0xC9, 0x22, 0x31, 0x96}};
    ComPtr<IUnknown> ksc;
    CHECK(source->QueryInterface(IID_KS, &ksc), "QI IKsControl");

    ComPtr<IMFPresentationDescriptor> pd;
    CHECK(source->CreatePresentationDescriptor(&pd), "CreatePresentationDescriptor");
    BOOL sel; ComPtr<IMFStreamDescriptor> sd;
    CHECK(pd->GetStreamDescriptorByIndex(0, &sel, &sd), "GetStreamDescriptorByIndex");
    ComPtr<IMFMediaTypeHandler> h;
    sd->GetMediaTypeHandler(&h);
    DWORD types = 0; h->GetMediaTypeCount(&types);
    std::cout << "     media types: " << types << "\n";
    ComPtr<IMFMediaType> t720;
    CHECK(h->GetMediaTypeByIndex(1, &t720), "GetMediaTypeByIndex(1)");
    CHECK(h->SetCurrentMediaType(t720.Get()), "SetCurrentMediaType 1280x720");

    PROPVARIANT start; PropVariantInit(&start);
    CHECK(source->Start(pd.Get(), nullptr, &start), "Start");

    ComPtr<IMFMediaStream> stream;
    for (int i = 0; i < 2; i++) {
        ComPtr<IMFMediaEvent> ev;
        CHECK(source->GetEvent(0, &ev), "source GetEvent");
        MediaEventType type; ev->GetType(&type);
        std::cout << "     event " << type << "\n";
        if (type == MENewStream) {
            PROPVARIANT v; PropVariantInit(&v); ev->GetValue(&v);
            v.punkVal->QueryInterface(IID_PPV_ARGS(&stream));
            PropVariantClear(&v);
        }
    }
    if (!stream) { std::cout << "FAIL no stream\n"; return 1; }
    ComPtr<IMFMediaEvent> started;
    CHECK(stream->GetEvent(0, &started), "stream MEStreamStarted");

    LONGLONG first = 0, last = 0;
    for (int i = 0; i < 10; i++) {
        CHECK(stream->RequestSample(nullptr), "RequestSample");
        ComPtr<IMFMediaEvent> ev;
        CHECK(stream->GetEvent(0, &ev), "stream GetEvent");
        MediaEventType type; ev->GetType(&type);
        if (type != MEMediaSample) { std::cout << "FAIL event " << type << "\n"; return 1; }
        PROPVARIANT v; PropVariantInit(&v); ev->GetValue(&v);
        ComPtr<IMFSample> sample; v.punkVal->QueryInterface(IID_PPV_ARGS(&sample)); PropVariantClear(&v);
        DWORD len = 0; sample->GetTotalLength(&len);
        LONGLONG ts = 0; sample->GetSampleTime(&ts);
        if (!i) first = ts; last = ts;
        if (i == 0) std::cout << "     sample bytes " << std::dec << len << " (expect " << 1280 * 720 * 3 / 2 << ")\n";
    }
    std::cout << "     10 samples over " << std::dec << (last - first) / 10000 << " ms\n";
    CHECK(source->Stop(), "Stop");
    CHECK(source->Shutdown(), "Shutdown");
    std::cout << "ALL OK\n";
    return 0;
}
