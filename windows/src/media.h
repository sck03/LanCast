#pragma once
#include <functional>
#include <memory>
#include <nlohmann/json.hpp>
#include <string>
#include <windows.h>
class MediaSender {
  public:
    using Json = nlohmann::json;
    using Event = std::function<void(std::string, Json)>;
    static bool available();
    explicit MediaSender(Event event);
    ~MediaSender();
    MediaSender(const MediaSender &) = delete;
    MediaSender &operator=(const MediaSender &) = delete;
    // Native calls run on one owner thread. started means creation succeeded, not first frame.
    void start(HWND window, bool audio, const Json &profile);
    void start_live(HWND window, HMONITOR monitor, bool audio, bool synthetic,
                    std::function<int32_t(const uint8_t *, size_t)> sink);
    void answer(const std::string &sdp, const std::string &negotiation);
    void ice(const Json &body);
    void restart(const Json &body);
    // Nonblocking; media.stopped arrives only after native destruction and callback exit.
    void stop();

  private:
    struct State;
    std::shared_ptr<State> state_;
};
