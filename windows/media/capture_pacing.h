#pragma once
#include <algorithm>
#include <cstdint>

namespace lancast {
// DLNA requires continuous TS video. RTC repeats static content only for recovery/keepalive.
constexpr bool repeat_capture(bool live, bool keyframe, int64_t now, int64_t last_frame) {
    return live || keyframe || now - last_frame >= 1000000;
}
constexpr int64_t capture_delay_us(int64_t now, int64_t next_video, bool audio) {
    return std::clamp<int64_t>(next_video - now, 1000, audio ? 10000 : 33000);
}
} // namespace lancast
