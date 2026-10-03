#include "rtc_channel.h"
#include <atomic>
#include <chrono>
#include <iostream>
#include <thread>

using Json = nlohmann::json;
struct Signals {
    std::mutex mutex;
    std::vector<Json> to_sender, to_receiver;
    std::vector<int> tracks;
    std::string negotiation;
    std::atomic_int packets{0}, keyframes{0};
};
int main() {
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    Signals signals;
    int receiver = -1;
    try {
        rtcConfiguration config{};
        config.disableAutoNegotiation = true;
        receiver = rtcCreatePeerConnection(&config);
        if (receiver < 0)
            throw std::runtime_error("receiver creation failed");
        rtcSetUserPointer(receiver, &signals);
        rtcSetLocalDescriptionCallback(receiver, [](int, const char *sdp, const char *, void *p) {
            auto &s = *static_cast<Signals *>(p);
            std::lock_guard lock(s.mutex);
            s.to_sender.push_back({{"type", "rtc.answer"},
                                   {"body", {{"sdp", sdp}, {"negotiationId", s.negotiation}}}});
        });
        rtcSetLocalCandidateCallback(receiver, [](int, const char *candidate, const char *mid,
                                                  void *p) {
            auto &s = *static_cast<Signals *>(p);
            std::lock_guard lock(s.mutex);
            s.to_sender.push_back(
                {{"type", "rtc.ice"},
                 {"body",
                  {{"candidate", candidate}, {"sdpMid", mid}, {"negotiationId", s.negotiation}}}});
        });
        rtcSetTrackCallback(receiver, [](int, int track, void *p) {
            auto &s = *static_cast<Signals *>(p);
            rtcSetUserPointer(track, p);
            rtcSetMessageCallback(track, [](int, const char *bytes, int size, void *user) {
                // This verifies transport/decryption and RTP framing, not H.264 decoding.
                if (size >= 12 && (static_cast<unsigned char>(bytes[0]) >> 6) == 2)
                    ++static_cast<Signals *>(user)->packets;
            });
            rtcChainRtcpReceivingSession(track);
            std::lock_guard lock(s.mutex);
            s.tracks.push_back(track);
        });
        {
            lancast::RtcChannel sender(
                true,
                [&](std::string type, Json body) {
                    std::lock_guard lock(signals.mutex);
                    signals.to_receiver.push_back({{"type", type}, {"body", body}});
                },
                [&] { ++signals.keyframes; }, [](uint32_t) {});
            const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(15);
            int64_t timestamp = 0;
            bool offer = false;
            std::vector<Json> remote_ice;
            while (signals.packets < 4 && std::chrono::steady_clock::now() < deadline) {
                std::vector<Json> outgoing, incoming;
                {
                    std::lock_guard lock(signals.mutex);
                    outgoing.swap(signals.to_receiver);
                    incoming.swap(signals.to_sender);
                }
                for (const auto &event : outgoing) {
                    const auto &b = event.at("body");
                    if (event.at("type") == "rtc.offer") {
                        {
                            std::lock_guard lock(signals.mutex);
                            signals.negotiation = b.at("negotiationId");
                        }
                        if (rtcSetRemoteDescription(
                                receiver, b.at("sdp").get<std::string>().c_str(), "offer") < 0 ||
                            rtcSetLocalDescription(receiver, "answer") < 0)
                            throw std::runtime_error("SDP interoperability failed");
                        offer = true;
                    } else if (event.at("type") == "rtc.ice")
                        remote_ice.push_back(b);
                    else if (event.at("type") == "media.error")
                        throw std::runtime_error("transport failed");
                }
                if (offer) {
                    for (const auto &ice : remote_ice)
                        rtcAddRemoteCandidate(receiver,
                                              ice.at("candidate").get<std::string>().c_str(),
                                              ice.at("sdpMid").get<std::string>().c_str());
                    remote_ice.clear();
                }
                for (const auto &event : incoming)
                    sender.command(event);
                uint8_t nal[]{0, 0, 0, 1, 0x65, 0x88, 0x84, 0x21, 0xa0};
                uint8_t opus[]{0xf8, 0xff, 0xfe};
                sender.send({lancast::Codec::H264, nal, timestamp, true});
                sender.send({lancast::Codec::Opus, opus, timestamp, false});
                timestamp += 20000;
                std::this_thread::sleep_for(std::chrono::milliseconds(20));
            }
            if (signals.packets < 4 || signals.keyframes < 1)
                throw std::runtime_error("no decrypted RTP received");
            bool rejected = false;
            try {
                sender.command({{"type", "rtc.ice"}, {"body", {{"negotiationId", "stale"}}}});
            } catch (...) {
                rejected = true;
            }
            if (!rejected)
                throw std::runtime_error("stale negotiation accepted");
        }
        rtcClosePeerConnection(receiver);
        for (int track : signals.tracks)
            rtcDeleteTrack(track);
        rtcDeletePeerConnection(receiver);
        receiver = -1;
        rtcCleanup();
        CoUninitialize();
        std::cout << "PASS: local ICE/DTLS/SRTP, H264/Opus RTP, stale negotiation and shutdown\n";
        return 0;
    } catch (const std::exception &e) {
        std::cerr << e.what() << '\n';
        if (receiver >= 0)
            rtcDeletePeerConnection(receiver);
        rtcCleanup();
        CoUninitialize();
        return 1;
    }
}
