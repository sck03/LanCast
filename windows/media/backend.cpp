#include "lancast_rtc.h"
#include "capture.h"
#include "rtc_channel.h"
#include "ts_output.h"
#include <map>
#include <mutex>
namespace {
struct Backend {
    LcRtcConfig config;
    std::atomic_bool active{true}; std::atomic_bool keyframe{true}; std::atomic_uint bitrate;
    std::unique_ptr<lancast::RtcChannel> rtc;
    std::unique_ptr<lancast::TsOutput> ts;
    std::unique_ptr<lancast::Capture> capture;
    explicit Backend(const LcRtcConfig& c) : config(c), bitrate(c.bitrate) {}
    void emit(std::string type, nlohmann::json body) noexcept {
        try { if (active && config.event) { const auto text = nlohmann::json{{"type",type},{"body",body}}.dump(); config.event(config.user, reinterpret_cast<const uint8_t*>(text.data()), text.size()); } } catch (...) {}
    }
    void start() {
        lancast::CaptureConfig c; c.window = reinterpret_cast<HWND>(config.window_handle); c.width = config.width; c.height = config.height;
        c.monitor = reinterpret_cast<HMONITOR>(config.monitor_handle);
        c.fps = config.fps; c.bitrate = config.bitrate; c.audio = config.audio != 0; c.synthetic = config.synthetic != 0; c.live = config.route == 1;
        if (c.live) ts = std::make_unique<lancast::TsOutput>(c, config.write_ts, config.ts_user);
        else rtc = std::make_unique<lancast::RtcChannel>(c.audio, [this](auto type, auto body) { emit(type, body); }, [this] { keyframe = true; }, [this](uint32_t value) { bitrate = value; });
        capture = std::make_unique<lancast::Capture>(c, [this](const lancast::Packet& p) {
            if (!active) return;
            if (capture) { if (keyframe.exchange(false)) capture->request_keyframe(); capture->set_bitrate(bitrate); }
            if (rtc) rtc->send(p); else ts->send(p);
        }, [this](std::string reason) { emit("media.error", {{"code",reason}}); });
    }
    ~Backend() { active = false; capture.reset(); rtc.reset(); ts.reset(); }
};
std::mutex registry_mutex;
std::map<LcRtcHandle, std::shared_ptr<Backend>> registry;
LcRtcHandle next_handle = 0;
}
extern "C" __declspec(dllexport) uint32_t lc_rtc_version() { return 2; }
extern "C" __declspec(dllexport) LcRtcHandle lc_rtc_create(const LcRtcConfig* c) {
    try {
        if (!c || c->size < sizeof(*c) || c->abi_version != 2 || !c->event || c->route > 1 || (c->route == 1 && !c->write_ts)) return 0;
        auto backend = std::make_shared<Backend>(*c); backend->start();
        std::lock_guard lock(registry_mutex); const auto handle = ++next_handle; registry.emplace(handle, std::move(backend)); return handle;
    } catch (...) { return 0; }
}
extern "C" __declspec(dllexport) int32_t lc_rtc_command(LcRtcHandle h, const uint8_t* bytes, size_t length) {
    try {
        if (!bytes || !length || length > 128 * 1024) return -1;
        std::shared_ptr<Backend> b; { std::lock_guard lock(registry_mutex); auto it = registry.find(h); if (it == registry.end()) return -2; b = it->second; }
        if (b->rtc) b->rtc->command(nlohmann::json::parse(bytes, bytes + length)); else return -1;
        return 0;
    } catch (...) { return -1; }
}
extern "C" __declspec(dllexport) void lc_rtc_destroy(LcRtcHandle h) {
    std::shared_ptr<Backend> b; { std::lock_guard lock(registry_mutex); auto it = registry.find(h); if (it == registry.end()) return; b = std::move(it->second); registry.erase(it); }
}
