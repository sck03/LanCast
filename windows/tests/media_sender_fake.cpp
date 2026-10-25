#include "lancast_rtc.h"
#include <chrono>
#include <thread>
#include <cstring>

static LcRtcConfig config;

extern "C" __declspec(dllexport) uint32_t lc_rtc_version() {
    return 2;
}
extern "C" __declspec(dllexport) LcRtcHandle lc_rtc_create(const LcRtcConfig *value) {
    config = *value;
    const char entered[] = R"({"type":"mock.creating","body":{}})";
    config.event(config.user, reinterpret_cast<const uint8_t *>(entered), std::strlen(entered));
    std::this_thread::sleep_for(std::chrono::milliseconds(180));
    return 1;
}
extern "C" __declspec(dllexport) int32_t lc_rtc_command(LcRtcHandle handle, const uint8_t *, size_t) {
    const char entered[] = R"({"type":"mock.command","body":{}})";
    config.event(config.user, reinterpret_cast<const uint8_t *>(entered), std::strlen(entered));
    std::this_thread::sleep_for(std::chrono::milliseconds(180));
    return handle == 1 ? 0 : -2;
}
extern "C" __declspec(dllexport) void lc_rtc_destroy(LcRtcHandle) {
    std::this_thread::sleep_for(std::chrono::milliseconds(180));
}
