#pragma once
#include "types.h"
#include <mutex>
#include <nlohmann/json.hpp>
#include <rtc/rtc.h>
namespace lancast {
class RtcChannel {
  public:
    using Event = std::function<void(std::string, nlohmann::json)>;
    RtcChannel(bool audio, Event event, std::function<void()> keyframe,
               std::function<void(uint32_t)> bitrate);
    ~RtcChannel();
    void command(const nlohmann::json &message);
    void send(const Packet &packet);

  private:
    int add_track(bool audio);
    void emit(std::string type, nlohmann::json body) noexcept;
    int pc_ = -1, video_ = -1, audio_ = -1;
    Event event_;
    std::function<void()> keyframe_;
    std::function<void(uint32_t)> bitrate_;
    std::string negotiation_;
    std::mutex mutex_;
    bool answered_ = false;
    std::vector<nlohmann::json> remote_ice_;
};
} // namespace lancast
