#include "media.h"
#include "lancast_rtc.h"
#include <atomic>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <stdexcept>
#include <thread>
static HMODULE load() {
    // Only the explicit sibling build artifact; never search PATH or current directory.
    wchar_t path[32768];
    const auto n = GetModuleFileNameW(nullptr, path, 32768);
    if (!n || n >= 32768)
        return nullptr;
    std::wstring file(path, n);
    file.resize(file.find_last_of(L"\\/") + 1);
    file += L"lancast_rtc.dll";
    return LoadLibraryExW(file.c_str(), nullptr,
                          LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
}
struct MediaSender::State {
    Event event;
    HMODULE module = nullptr;
    LcRtcHandle handle = 0;
    LcRtcCreateFn create = nullptr;
    LcRtcCommandFn command = nullptr;
    LcRtcDestroyFn destroy = nullptr;
    std::atomic_bool active{false};
    bool launched = false;
    std::function<int32_t(const uint8_t *, size_t)> ts;
    std::mutex mutex;
    std::condition_variable ready;
    std::deque<Json> commands;
    void release() {
        if (handle && destroy)
            destroy(handle);
        handle = 0;
        if (module)
            FreeLibrary(module);
        module = nullptr;
    }
    void emit(const std::string &type, Json body = Json::object()) noexcept {
        try {
            event(type, std::move(body));
        } catch (...) {
        }
    }
    void send(Json message) {
        std::lock_guard lock(mutex);
        if (!active)
            return;
        if (commands.size() == 32)
            throw std::runtime_error("RTC_COMMAND_QUEUE_FULL");
        commands.push_back(std::move(message));
        ready.notify_one();
    }
    void run(LcRtcConfig config) noexcept {
        try {
            module = load();
            if (!module)
                throw std::runtime_error("MEDIA_LIBRARY_MISSING");
            auto version = reinterpret_cast<LcRtcVersionFn>(GetProcAddress(module, "lc_rtc_version"));
            create = reinterpret_cast<LcRtcCreateFn>(GetProcAddress(module, "lc_rtc_create"));
            command = reinterpret_cast<LcRtcCommandFn>(GetProcAddress(module, "lc_rtc_command"));
            destroy = reinterpret_cast<LcRtcDestroyFn>(GetProcAddress(module, "lc_rtc_destroy"));
            if (!version || version() != 2 || !create || !command || !destroy)
                throw std::runtime_error("RTC_ABI_MISMATCH");
            if (active) {
                handle = create(&config);
                if (!handle)
                    throw std::runtime_error("RTC_INITIALIZATION_FAILED");
            }
            if (active)
                emit("media.started");
            while (active) {
                Json message;
                {
                    std::unique_lock lock(mutex);
                    ready.wait(lock, [this] { return !active || !commands.empty(); });
                    if (!active)
                        break;
                    message = std::move(commands.front());
                    commands.pop_front();
                }
                auto text = message.dump();
                if (command(handle, reinterpret_cast<const uint8_t *>(text.data()), text.size()) != 0)
                    throw std::runtime_error("RTC_COMMAND_FAILED");
            }
        } catch (const std::exception &error) {
            if (active)
                emit("media.error", {{"code", error.what()}});
        } catch (...) {
            if (active)
                emit("media.error", {{"code", "MEDIA_FAILED"}});
        }
        active = false;
        release();
        emit("media.stopped");
    }
};
bool MediaSender::available() {
    auto module = load();
    if (!module)
        return false;
    auto version = reinterpret_cast<LcRtcVersionFn>(GetProcAddress(module, "lc_rtc_version"));
    bool valid = version && version() == 2 && GetProcAddress(module, "lc_rtc_create") &&
                 GetProcAddress(module, "lc_rtc_command") &&
                 GetProcAddress(module, "lc_rtc_destroy");
    FreeLibrary(module);
    return valid;
}
MediaSender::MediaSender(Event event) : state_(std::make_shared<State>()) {
    state_->event = std::move(event);
}
MediaSender::~MediaSender() {
    stop();
}
void MediaSender::start(HWND window, bool audio, const Json &profile) {
    if (!state_)
        throw std::runtime_error("MEDIA_CLOSED");
    auto &s = *state_;
    if (s.launched)
        throw std::runtime_error("ALREADY_ACTIVE");
    LcRtcConfig c{};
    c.size = sizeof(c);
    c.abi_version = 2;
    c.window_handle = reinterpret_cast<uintptr_t>(window);
    c.width = profile.value("width", 1280);
    c.height = profile.value("height", 720);
    c.fps = profile.value("fps", 30);
    c.bitrate = profile.value("bitrate", 3000000);
    c.audio = audio;
    c.route = profile.value("live", false) ? 1 : 0;
    c.synthetic = profile.value("synthetic", false) ? 1 : 0;
    c.monitor_handle = profile.value("monitor", uint64_t{0});
    c.ts_user = &s;
    c.write_ts = +[](void *user, const uint8_t *bytes, size_t size) -> int32_t {
        auto *state = static_cast<State *>(user);
        return state->active && state->ts ? state->ts(bytes, size) : -2;
    };
    c.user = &s;
    c.event = +[](void *user, const uint8_t *data, size_t size) {
        auto *state = static_cast<State *>(user);
        if (!state->active || !data || size > 128 * 1024)
            return;
        try {
            auto event = Json::parse(data, data + size);
            state->event(event.at("type").get<std::string>(), event.value("body", Json::object()));
        } catch (...) {
        }
    };
    s.active = true;
    s.launched = true;
    try {
        // One owner serializes create, commands and destroy away from the window thread.
        // Its shared state remains alive until the native destroy call has joined callbacks.
        std::thread([state = state_, c] { state->run(c); }).detach();
    } catch (...) {
        s.active = false;
        s.launched = false;
        throw;
    }
}
void MediaSender::start_live(HWND window, HMONITOR monitor, bool audio, bool synthetic,
                             std::function<int32_t(const uint8_t *, size_t)> sink) {
    if (!state_)
        throw std::runtime_error("MEDIA_CLOSED");
    state_->ts = std::move(sink);
    start(window, audio,
          {{"live", true},
           {"synthetic", synthetic},
           {"monitor", reinterpret_cast<uintptr_t>(monitor)}});
}
void MediaSender::answer(const std::string &sdp, const std::string &negotiation) {
    if (!state_)
        return;
    state_->send(
        {{"type", "rtc.answer"}, {"body", {{"sdp", sdp}, {"negotiationId", negotiation}}}});
}
void MediaSender::ice(const Json &body) {
    if (state_)
        state_->send({{"type", "rtc.ice"}, {"body", body}});
}
void MediaSender::restart(const Json &body) {
    if (state_)
        state_->send({{"type", "rtc.restart"}, {"body", body}});
}
void MediaSender::stop() {
    if (!state_)
        return;
    state_->active = false;
    {
        std::lock_guard lock(state_->mutex);
        state_->commands.clear();
    }
    state_->ready.notify_one();
    if (!state_->launched)
        state_->emit("media.stopped");
    state_.reset();
}
