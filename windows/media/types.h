#pragma once
#include <cstdint>
#include <functional>
#include <span>
#include <stdexcept>
#include <string>
#include <windows.h>

namespace lancast {
// Encoded packets are borrowed for the duration of the callback. No pixels leave this module.
enum class Codec { H264, Aac, Opus };
struct Packet {
    Codec codec;
    std::span<const uint8_t> bytes;
    int64_t time_us;
    bool keyframe;
};
struct CaptureConfig {
    HWND window = nullptr;
    HMONITOR monitor = nullptr;
    uint32_t width = 1280, height = 720, fps = 30, bitrate = 3000000;
    bool audio = true, synthetic = false, live = false;
};
using PacketSink = std::function<void(const Packet &)>;
using Failure = std::function<void(std::string)>;
inline void check(HRESULT hr, const char *operation) {
    if (FAILED(hr))
        throw std::runtime_error(std::string(operation) + " (HRESULT " +
                                 std::to_string(static_cast<uint32_t>(hr)) + ")");
}
inline int64_t clock_us() {
    LARGE_INTEGER ticks{}, frequency{};
    QueryPerformanceCounter(&ticks);
    QueryPerformanceFrequency(&frequency);
    return ticks.QuadPart / frequency.QuadPart * 1000000 +
           ticks.QuadPart % frequency.QuadPart * 1000000 / frequency.QuadPart;
}
} // namespace lancast
