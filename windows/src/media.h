#pragma once
#include <windows.h>
#include <nlohmann/json.hpp>
#include <functional>
#include <memory>
#include <string>
class MediaSender {
public:
    using Json = nlohmann::json;
    using Event = std::function<void(std::string, Json)>;
    static bool available();
    explicit MediaSender(Event event);
    ~MediaSender();
    MediaSender(const MediaSender&) = delete;
    MediaSender& operator=(const MediaSender&) = delete;
    void start(HWND window, bool audio, const Json& profile);
    void start_live(HWND window, HMONITOR monitor, bool audio, bool synthetic, std::function<int32_t(const uint8_t*, size_t)> sink);
    void answer(const std::string& sdp, const std::string& negotiation);
    void ice(const Json& body);
    void pump() {}
    void stop();
private:
    struct State;
    std::shared_ptr<State> state_;
};
