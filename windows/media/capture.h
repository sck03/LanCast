#pragma once
#include "types.h"
#include <atomic>
#include <memory>
namespace lancast {
class Capture {
public:
    Capture(CaptureConfig config, PacketSink sink, Failure failure);
    ~Capture();
    Capture(const Capture&) = delete;
    Capture& operator=(const Capture&) = delete;
    void request_keyframe();
    void set_bitrate(uint32_t bitrate);
    void stop();
private:
    struct State;
    std::unique_ptr<State> state_;
};
}
