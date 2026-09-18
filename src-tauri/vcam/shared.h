// Contract shared by AndroidToolsCam.dll, atcam.exe and the Python frame writer (atools/vcam.py).
#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>

// {6B1F0C2A-8E4D-4F7B-A3C5-2D9E7F10B4A6}
static const CLSID CLSID_AndroidToolsCam =
    {0x6b1f0c2a, 0x8e4d, 0x4f7b, {0xa3, 0xc5, 0x2d, 0x9e, 0x7f, 0x10, 0xb4, 0xa6}};
#define AT_CLSID_STR L"{6B1F0C2A-8E4D-4F7B-A3C5-2D9E7F10B4A6}"
#define AT_CAMERA_NAME L"Android Tools Camera"

// Shared memory with the latest NV12 frame. Writer uses a seqlock: counter is odd while writing.
#define AT_MAP_NAME L"Global\\AndroidToolsCamFrame"
#define AT_MAP_SDDL L"D:(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;BA)(A;;GRGW;;;AU)"
#define AT_MAGIC 0x4D435441u  // "ATCM"
#define AT_MAX_WIDTH 1920
#define AT_MAX_HEIGHT 1080
#define AT_HEADER_SIZE 64
#define AT_MAP_SIZE (AT_HEADER_SIZE + AT_MAX_WIDTH * AT_MAX_HEIGHT * 3 / 2)

#pragma pack(push, 1)
struct AtFrameHeader {
    UINT32 magic;
    UINT32 version;
    UINT32 width;
    UINT32 height;
    UINT64 counter;
    UINT64 tick;      // GetTickCount64() of the last write
    BYTE reserved[32];
};
#pragma pack(pop)
