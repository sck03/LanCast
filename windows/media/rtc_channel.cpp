#include "rtc_channel.h"
#include <atomic>
#include <map>
#include <objbase.h>
namespace lancast {
namespace {
// The C API hands callbacks a copied raw user pointer; deletion alone does not wait for it.
// Pass an opaque integer and resolve it under a callback gate instead of dereferencing a dying
// owner.
std::mutex callback_mutex;
std::map<uintptr_t, RtcChannel *> callback_owners;
uintptr_t next_callback = 0;
uintptr_t register_callback(RtcChannel *owner) {
    std::lock_guard lock(callback_mutex);
    const auto token = ++next_callback;
    callback_owners.emplace(token, owner);
    return token;
}
void unregister_callback(uintptr_t token) {
    std::lock_guard lock(callback_mutex);
    callback_owners.erase(token);
}
template <class Fn> void with_user(void *pointer, Fn &&fn) noexcept {
    try {
        std::lock_guard lock(callback_mutex);
        const auto it = callback_owners.find(reinterpret_cast<uintptr_t>(pointer));
        if (it != callback_owners.end())
            fn(*it->second);
    } catch (...) {
    }
}
} // namespace
static void rtc_check(int result) {
    if (result < 0)
        throw std::runtime_error("RTC_OPERATION_FAILED");
}
static uint32_t random_ssrc() {
    GUID id{};
    check(CoCreateGuid(&id), "RANDOM_FAILED");
    return id.Data1 ? id.Data1 : 1;
}
RtcChannel::RtcChannel(bool audio, Event event, std::function<void()> keyframe,
                       std::function<void(uint32_t)> bitrate)
    : event_(std::move(event)), keyframe_(std::move(keyframe)), bitrate_(std::move(bitrate)) {
    GUID guid{};
    check(CoCreateGuid(&guid), "NEGOTIATION_ID_FAILED");
    wchar_t wide[40];
    StringFromGUID2(guid, wide, 40);
    for (int i = 1; i < 37; ++i)
        negotiation_.push_back(static_cast<char>(wide[i]));
    rtcConfiguration c{};
    c.disableAutoNegotiation = true;
    c.forceMediaTransport = true;
    pc_ = rtcCreatePeerConnection(&c);
    rtc_check(pc_);
    try {
        callback_token_ = register_callback(this);
        rtcSetUserPointer(pc_, reinterpret_cast<void *>(callback_token_));
        rtc_check(
            rtcSetLocalDescriptionCallback(pc_, [](int, const char *sdp, const char *, void *user) {
                with_user(user, [&](RtcChannel &self) {
                    self.emit("rtc.offer", {{"sdp", sdp}, {"negotiationId", self.negotiation_}});
                    self.description_sent_ = true;
                    for (auto &ice : self.local_ice_)
                        self.emit("rtc.ice", std::move(ice));
                    self.local_ice_.clear();
                });
            }));
        rtc_check(rtcSetLocalCandidateCallback(
            pc_, [](int, const char *candidate, const char *mid, void *user) {
                with_user(user, [&](RtcChannel &self) {
                    nlohmann::json ice{{"candidate", candidate},
                                       {"sdpMid", mid},
                                       {"sdpMLineIndex", std::string(mid) == "audio" ? 1 : 0},
                                       {"negotiationId", self.negotiation_}};
                    if (self.description_sent_)
                        self.emit("rtc.ice", std::move(ice));
                    else if (self.local_ice_.size() < 64)
                        self.local_ice_.push_back(std::move(ice));
                    else
                        self.emit("media.error", {{"code", "ICE_QUEUE_FULL"}});
                });
            }));
        rtc_check(rtcSetStateChangeCallback(pc_, [](int, rtcState state, void *user) {
            with_user(user, [&](RtcChannel &self) {
                if (state == RTC_CONNECTED) {
                    self.keyframe_();
                    self.emit("media.connected", {{"transport", "dtls_srtp"}});
                } else if (state == RTC_FAILED || state == RTC_DISCONNECTED)
                    self.emit("media.error", {{"code", "RTC_DISCONNECTED"}});
            });
        }));
        video_ = add_track(false);
        if (audio)
            audio_ = add_track(true);
        rtc_check(rtcSetLocalDescription(pc_, "offer"));
    } catch (...) {
        unregister_callback(callback_token_);
        if (video_ >= 0)
            rtcDeleteTrack(video_);
        if (audio_ >= 0)
            rtcDeleteTrack(audio_);
        rtcDeletePeerConnection(pc_);
        throw;
    }
}
int RtcChannel::add_track(bool audio) {
    const auto ssrc = random_ssrc();
    const char *name = audio ? "audio" : "video";
    rtcTrackInit t{};
    t.direction = RTC_DIRECTION_SENDONLY;
    t.codec = audio ? RTC_CODEC_OPUS : RTC_CODEC_H264;
    t.payloadType = audio ? 111 : 102;
    t.ssrc = ssrc;
    t.mid = name;
    t.name = "lancast";
    t.msid = "lancast";
    t.trackId = name;
    t.profile = audio ? "minptime=10;useinbandfec=1;stereo=1"
                      : "profile-level-id=42e01f;packetization-mode=1;level-asymmetry-allowed=1";
    const auto track = rtcAddTrackEx(pc_, &t);
    rtc_check(track);
    try {
        rtcSetUserPointer(track, reinterpret_cast<void *>(callback_token_));
        rtcPacketizerInit p{};
        p.ssrc = ssrc;
        p.cname = "lancast";
        p.payloadType = static_cast<uint8_t>(t.payloadType);
        p.clockRate = audio ? 48000 : 90000;
        p.nalSeparator = RTC_NAL_SEPARATOR_START_SEQUENCE;
        p.maxFragmentSize = 1100;
        p.sequenceNumber = static_cast<uint16_t>(random_ssrc());
        rtc_check(audio ? rtcSetOpusPacketizer(track, &p) : rtcSetH264Packetizer(track, &p));
        rtc_check(rtcChainRtcpSrReporter(track));
        rtc_check(rtcChainRtcpNackResponder(track, 256));
        if (!audio) {
            rtc_check(rtcChainPliHandler(track, [](int, void *user) {
                with_user(user, [](RtcChannel &self) { self.keyframe_(); });
            }));
            rtc_check(rtcChainRembHandler(track, [](int, unsigned bitrate, void *user) {
                with_user(user, [&](RtcChannel &self) { self.bitrate_(bitrate); });
            }));
        }
        return track;
    } catch (...) {
        rtcDeleteTrack(track);
        throw;
    }
}
void RtcChannel::emit(std::string type, nlohmann::json body) noexcept {
    try {
        event_(std::move(type), std::move(body));
    } catch (...) {
    }
}
RtcChannel::~RtcChannel() {
    // Removing the token waits for in-flight callbacks and prevents late callbacks finding us.
    unregister_callback(callback_token_);
    if (pc_ >= 0)
        rtcClosePeerConnection(pc_);
    if (video_ >= 0)
        rtcDeleteTrack(video_);
    if (audio_ >= 0)
        rtcDeleteTrack(audio_);
    if (pc_ >= 0)
        rtcDeletePeerConnection(pc_);
}
void RtcChannel::command(const nlohmann::json &message) {
    std::lock_guard lock(mutex_);
    const auto &body = message.at("body");
    if (body.at("negotiationId").get<std::string>() != negotiation_)
        throw std::runtime_error("STALE_NEGOTIATION");
    auto add = [this](const auto &ice) {
        rtc_check(rtcAddRemoteCandidate(pc_,
                                        ice.at("candidate").template get<std::string>().c_str(),
                                        ice.at("sdpMid").template get<std::string>().c_str()));
    };
    if (message.at("type") == "rtc.answer") {
        if (answered_)
            throw std::runtime_error("DUPLICATE_ANSWER");
        rtc_check(
            rtcSetRemoteDescription(pc_, body.at("sdp").get<std::string>().c_str(), "answer"));
        answered_ = true;
        for (const auto &ice : remote_ice_)
            add(ice);
        remote_ice_.clear();
    } else if (message.at("type") == "rtc.ice") {
        if (answered_)
            add(body);
        else {
            if (remote_ice_.size() >= 64)
                throw std::runtime_error("ICE_QUEUE_FULL");
            remote_ice_.push_back(body);
        }
    } else
        throw std::runtime_error("UNKNOWN_RTC_COMMAND");
}
void RtcChannel::send(const Packet &packet) {
    const int track = packet.codec == Codec::H264 ? video_ : audio_;
    if (track < 0 || !rtcIsOpen(track))
        return;
    if (rtcGetBufferedAmount(track) > 512 * 1024)
        throw std::runtime_error("RTC_BACKPRESSURE");
    const uint64_t rate = packet.codec == Codec::H264 ? 90000 : 48000;
    rtc_check(rtcSetTrackRtpTimestamp(
        track, static_cast<uint32_t>(static_cast<uint64_t>(packet.time_us) * rate / 1000000)));
    rtc_check(rtcSendMessage(track, reinterpret_cast<const char *>(packet.bytes.data()),
                             static_cast<int>(packet.bytes.size())));
}
} // namespace lancast
