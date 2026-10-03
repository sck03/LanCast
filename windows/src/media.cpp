#include "media.h"
#include "lancast_rtc.h"
#include <atomic>
#include <mutex>
#include <stdexcept>
#include <thread>
static HMODULE load() {
    // Only the explicit sibling build artifact; never search PATH or current directory.
    wchar_t path[32768]; const auto n=GetModuleFileNameW(nullptr,path,32768);
    if(!n || n>=32768) return nullptr;
    std::wstring file(path,n); file.resize(file.find_last_of(L"\\/")+1); file+=L"lancast_rtc.dll";
    return LoadLibraryExW(file.c_str(),nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_SYSTEM32);
}
struct MediaSender::State {
    Event event; HMODULE module=nullptr; LcRtcHandle handle=0;
    LcRtcCreateFn create=nullptr; LcRtcCommandFn command=nullptr; LcRtcDestroyFn destroy=nullptr;
    std::atomic_bool active{false};
    ~State() { if(handle && destroy) destroy(handle); if(module) FreeLibrary(module); }
    void send(const Json& message) {
        if(!active || !handle) return;
        auto text=message.dump(); if(command(handle,reinterpret_cast<const uint8_t*>(text.data()),text.size())!=0) throw std::runtime_error("RTC_COMMAND_FAILED");
    }
};
bool MediaSender::available() {
    auto module=load(); if(!module) return false;
    auto version=reinterpret_cast<LcRtcVersionFn>(GetProcAddress(module,"lc_rtc_version"));
    bool valid=version && version()==1 && GetProcAddress(module,"lc_rtc_create") && GetProcAddress(module,"lc_rtc_command") && GetProcAddress(module,"lc_rtc_destroy");
    FreeLibrary(module); return valid;
}
MediaSender::MediaSender(Event event):state_(std::make_shared<State>()) { state_->event=std::move(event); }
MediaSender::~MediaSender() { stop(); }
void MediaSender::start(HWND window,bool audio,const Json& profile) {
    auto& s=*state_; if(s.active) throw std::runtime_error("ALREADY_ACTIVE");
    s.module=load(); if(!s.module) throw std::runtime_error("此构建未包含原生 RTC 媒体后端，屏幕分享不可用");
    auto version=reinterpret_cast<LcRtcVersionFn>(GetProcAddress(s.module,"lc_rtc_version"));
    s.create=reinterpret_cast<LcRtcCreateFn>(GetProcAddress(s.module,"lc_rtc_create"));
    s.command=reinterpret_cast<LcRtcCommandFn>(GetProcAddress(s.module,"lc_rtc_command"));
    s.destroy=reinterpret_cast<LcRtcDestroyFn>(GetProcAddress(s.module,"lc_rtc_destroy"));
    if(!version || version()!=1 || !s.create || !s.command || !s.destroy) throw std::runtime_error("RTC_ABI_MISMATCH");
    LcRtcConfig c{}; c.size=sizeof(c); c.abi_version=1; c.window_handle=reinterpret_cast<uintptr_t>(window);
    c.width=profile.value("width",1280); c.height=profile.value("height",720); c.fps=profile.value("fps",30); c.bitrate=profile.value("bitrate",3000000); c.audio=audio;
    c.user=&s; c.event=+[](void* user,const uint8_t* data,size_t size) {
        auto* state=static_cast<State*>(user); if(!state->active || !data || size>128*1024) return;
        try { auto event=Json::parse(data,data+size); state->event(event.at("type").get<std::string>(),event.value("body",Json::object())); } catch(...) {}
    };
    s.active=true; s.handle=s.create(&c);
    if(!s.handle) { s.active=false; throw std::runtime_error("RTC_INITIALIZATION_FAILED"); }
}
void MediaSender::answer(const std::string& sdp,const std::string& negotiation) { state_->send({{"type","rtc.answer"},{"body",{{"sdp",sdp},{"negotiationId",negotiation}}}}); }
void MediaSender::ice(const Json& body) { state_->send({{"type","rtc.ice"},{"body",body}}); }
void MediaSender::stop() {
    if(!state_) return;
    state_->active=false;
    // Callback owner remains alive until the backend has stopped every callback.
    auto state=std::move(state_); std::thread([state=std::move(state)]() mutable { state.reset(); }).detach();
}
