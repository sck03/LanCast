#pragma once
#include <windows.h>
#include <gst/gst.h>
#include <nlohmann/json.hpp>
#include <functional>
#include <memory>
#include <string>
class MediaSender {
public:
    using Json = nlohmann::json;
    using Event = std::function<void(std::string, Json)>;
    explicit MediaSender(Event event);
    ~MediaSender();
    MediaSender(const MediaSender&) = delete;
    MediaSender& operator=(const MediaSender&) = delete;
    void start(HWND window, bool audio, const Json& profile);
    void answer(const std::string& sdp, const std::string& negotiation);
    void ice(const Json& body);
    void pump();
    void stop();
private:
    struct Shared;
    std::shared_ptr<Shared> shared_;
    GstElement* pipeline_ = nullptr;
    GstElement* rtc_ = nullptr;
};
