#include "media.h"
#include <gst/webrtc/webrtc.h>
#include <gst/sdp/sdp.h>
#include <atomic>
#include <mutex>
#include <vector>
#include <stdexcept>

struct MediaSender::Shared {
    Event event;
    std::atomic_bool alive{true};
    std::mutex mutex;
    std::string negotiation;
    bool offered = false;
    bool remote = false;
    std::vector<Json> pending_local;
    std::vector<Json> pending_remote;
};
static std::string uuid() {
    GUID id; if (FAILED(CoCreateGuid(&id))) throw std::runtime_error("UUID_FAILED");
    wchar_t text[40]; StringFromGUID2(id, text, 40);
    std::string result; for (int i=1; i<37; ++i) result += static_cast<char>(text[i]);
    return result;
}
MediaSender::MediaSender(Event event): shared_(std::make_shared<Shared>()) {
    shared_->event = std::move(event);
}
MediaSender::~MediaSender() { stop(); }
void MediaSender::start(HWND window, bool audio, const Json& profile) {
    if (pipeline_) throw std::runtime_error("ALREADY_STREAMING");
    shared_->negotiation = uuid();
    for (const char* plugin : {"d3d11screencapturesrc","d3d11convert","mfh264enc","webrtcbin","rtph264pay"}) {
        auto* factory = gst_element_factory_find(plugin);
        if (!factory) throw std::runtime_error(std::string("MISSING_MEDIA_PLUGIN: ") + plugin);
        gst_object_unref(factory);
    }
    const int width = profile.value("width",1280), height = profile.value("height",720);
    const int fps = profile.value("fps",30), bitrate = profile.value("bitrate",3000000)/1000;
    const std::string pipeline =
        "webrtcbin name=rtc bundle-policy=max-bundle latency=100 "
        "d3d11screencapturesrc name=screen capture-api=wgc show-cursor=true ! "
        "queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream ! d3d11convert ! "
        "video/x-raw(memory:D3D11Memory),format=NV12,width=" + std::to_string(width) +
        ",height=" + std::to_string(height) + ",framerate=" + std::to_string(fps) + "/1 ! "
        "mfh264enc low-latency=true bitrate=" + std::to_string(bitrate) + " bframes=0 gop-size=60 ! "
        "video/x-h264,profile=constrained-baseline ! h264parse ! rtph264pay config-interval=-1 pt=96 ! "
        "application/x-rtp,media=video,encoding-name=H264,payload=96 ! rtc. " +
        (audio ? "wasapi2src loopback=true ! queue ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=2 ! opusenc bitrate=128000 ! rtpopuspay pt=97 ! application/x-rtp,media=audio,encoding-name=OPUS,payload=97 ! rtc." : "");
    GError* error = nullptr;
    pipeline_ = gst_parse_launch(pipeline.c_str(), &error);
    if (error) { std::string message = error->message; g_error_free(error); stop(); throw std::runtime_error(message); }
    if (!pipeline_) throw std::runtime_error("PIPELINE_FAILED");
    auto* source = gst_bin_get_by_name(GST_BIN(pipeline_), "screen");
    if (window) g_object_set(source, "window-handle", static_cast<guint64>(reinterpret_cast<uintptr_t>(window)), nullptr);
    gst_object_unref(source);
    rtc_ = gst_bin_get_by_name(GST_BIN(pipeline_), "rtc");
    // Each callback owns its shared state; late promises never touch a destroyed MediaSender.
    auto* ice_state = new std::shared_ptr<Shared>(shared_);
    g_signal_connect_data(rtc_, "on-ice-candidate", G_CALLBACK(+[](GstElement*, guint index, gchar* candidate, gpointer data) {
        auto state = *static_cast<std::shared_ptr<Shared>*>(data);
        if (!state->alive) return;
        Json body = {{"candidate",candidate},{"sdpMLineIndex",index},{"sdpMid",std::to_string(index)},{"negotiationId",state->negotiation}};
        std::lock_guard lock(state->mutex);
        if (state->offered) state->event("rtc.ice",body); else if (state->pending_local.size()<128) state->pending_local.push_back(body);
    }), ice_state, +[](gpointer data, GClosure*) { delete static_cast<std::shared_ptr<Shared>*>(data); }, GConnectFlags(0));
    auto* offer_state = new std::shared_ptr<Shared>(shared_);
    g_signal_connect_data(rtc_, "on-negotiation-needed", G_CALLBACK(+[](GstElement* rtc, gpointer data) {
        auto state = *static_cast<std::shared_ptr<Shared>*>(data);
        if (!state->alive) return;
        struct Offer { std::shared_ptr<Shared> state; GstElement* rtc; };
        auto* payload = new Offer{state, GST_ELEMENT(gst_object_ref(rtc))};
        auto* promise = gst_promise_new_with_change_func(+[](GstPromise* promise, gpointer data) {
            auto* payload = static_cast<Offer*>(data);
            auto state = payload->state;
            if (state->alive && gst_promise_wait(promise)==GST_PROMISE_RESULT_REPLIED) {
                const GstStructure* reply = gst_promise_get_reply(promise);
                GstWebRTCSessionDescription* offer = nullptr;
                if (reply && gst_structure_get(reply, "offer", GST_TYPE_WEBRTC_SESSION_DESCRIPTION, &offer, nullptr)) {
                    g_signal_emit_by_name(payload->rtc, "set-local-description", offer, nullptr);
                    auto* sdp = gst_sdp_message_as_text(offer->sdp);
                    {
                        std::lock_guard lock(state->mutex);
                        state->event("rtc.offer",{{"sdp",sdp},{"negotiationId",state->negotiation}});
                        state->offered = true;
                        for (const auto& ice : state->pending_local) state->event("rtc.ice",ice);
                        state->pending_local.clear();
                    }
                    g_free(sdp); gst_webrtc_session_description_free(offer);
                }
            }
            gst_promise_unref(promise);
        }, payload, +[](gpointer data) { auto* p=static_cast<Offer*>(data); gst_object_unref(p->rtc); delete p; });
        g_signal_emit_by_name(rtc, "create-offer", nullptr, promise);
    }), offer_state, +[](gpointer data, GClosure*) { delete static_cast<std::shared_ptr<Shared>*>(data); }, GConnectFlags(0));
    if (gst_element_set_state(pipeline_, GST_STATE_PLAYING)==GST_STATE_CHANGE_FAILURE) { stop(); throw std::runtime_error("HARDWARE_CAPTURE_OR_ENCODER_FAILED"); }
}
void MediaSender::answer(const std::string& sdp, const std::string& negotiation) {
    if (!rtc_ || negotiation!=shared_->negotiation) return;
    GstSDPMessage* message = nullptr; gst_sdp_message_new(&message);
    if (gst_sdp_message_parse_buffer(reinterpret_cast<const guint8*>(sdp.data()),static_cast<guint>(sdp.size()),message)!=GST_SDP_OK) {
        gst_sdp_message_free(message); throw std::runtime_error("INVALID_SDP");
    }
    auto* answer=gst_webrtc_session_description_new(GST_WEBRTC_SDP_TYPE_ANSWER,message);
    struct Pending { std::shared_ptr<Shared> state; GstElement* rtc; };
    auto* pending=new Pending{shared_,GST_ELEMENT(gst_object_ref(rtc_))};
    auto* promise=gst_promise_new_with_change_func(+[](GstPromise* promise,gpointer data){
        auto* p=static_cast<Pending*>(data);
        if (p->state->alive && gst_promise_wait(promise)==GST_PROMISE_RESULT_REPLIED) {
            std::lock_guard lock(p->state->mutex);p->state->remote=true;
            for (const auto& ice:p->state->pending_remote) g_signal_emit_by_name(p->rtc,"add-ice-candidate",ice["sdpMLineIndex"].get<guint>(),ice["candidate"].get<std::string>().c_str());
            p->state->pending_remote.clear();
        }
        gst_promise_unref(promise);
    },pending,+[](gpointer data){auto* p=static_cast<Pending*>(data);gst_object_unref(p->rtc);delete p;});
    g_signal_emit_by_name(rtc_,"set-remote-description",answer,promise);
    gst_webrtc_session_description_free(answer);
}
void MediaSender::ice(const Json& body) {
    if (!rtc_ || body.value("negotiationId","")!=shared_->negotiation) return;
    std::lock_guard lock(shared_->mutex);
    if (shared_->remote) g_signal_emit_by_name(rtc_,"add-ice-candidate",body["sdpMLineIndex"].get<guint>(),body["candidate"].get<std::string>().c_str());
    else if (shared_->pending_remote.size()<128) shared_->pending_remote.push_back(body);
}
void MediaSender::pump() {
    if (!pipeline_) return;
    auto* bus=gst_element_get_bus(pipeline_);
    while (auto* message=gst_bus_pop_filtered(bus,GstMessageType(GST_MESSAGE_ERROR|GST_MESSAGE_EOS))) {
        if (GST_MESSAGE_TYPE(message)==GST_MESSAGE_ERROR) {
            GError* error=nullptr;gchar* debug=nullptr;gst_message_parse_error(message,&error,&debug);
            shared_->event("media.error",{{"code",error?error->message:"MEDIA_FAILED"}});
            g_clear_error(&error);g_free(debug);
        } else shared_->event("media.ended",{});
        gst_message_unref(message);
    }
    gst_object_unref(bus);
}
void MediaSender::stop() {
    shared_->alive=false;
    if (pipeline_) gst_element_set_state(pipeline_,GST_STATE_NULL);
    if (rtc_) {gst_object_unref(rtc_);rtc_=nullptr;}
    if (pipeline_) {gst_object_unref(pipeline_);pipeline_=nullptr;}
}
