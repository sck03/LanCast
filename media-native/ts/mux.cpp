#include "lancast_media.h"
extern "C" {
#include <libavformat/avformat.h>
#include <libavutil/channel_layout.h>
#include <libavutil/mem.h>
}
#include <algorithm>
#include <cstring>
#include <limits>
#include <memory>
#include <vector>

struct LcTsMux {
    AVFormatContext* format = nullptr;
    AVIOContext* io = nullptr;
    LcTsConfig config{};
    std::vector<uint8_t> pending;
    int64_t last_video = -1, last_audio = -1;
    bool failed = false, header = false;
    ~LcTsMux() {
        if (format && header && !failed) { av_write_trailer(format); avio_flush(io); }
        if (format) { format->pb = nullptr; avformat_free_context(format); }
        if (io) { av_freep(&io->buffer); avio_context_free(&io); }
    }
};
static int output(void* opaque, const uint8_t* bytes, int count) {
    auto& m = *static_cast<LcTsMux*>(opaque);
    if (count < 0 || m.failed) return AVERROR_EXTERNAL;
    try {
        m.pending.insert(m.pending.end(), bytes, bytes + count);
        size_t used = 0;
        while (m.pending.size() - used >= 188) {
            const size_t n = std::min<size_t>((m.pending.size() - used) / 188 * 188, 65424);
            if (m.config.write(m.config.user, m.pending.data() + used, n) != 0) { m.failed = true; return AVERROR_EXTERNAL; }
            used += n;
        }
        m.pending.erase(m.pending.begin(), m.pending.begin() + used);
        return count;
    } catch (...) { m.failed = true; return AVERROR(ENOMEM); }
}
static bool nal_type(const uint8_t* data, size_t size, unsigned type) {
    for (size_t i = 0; i + 3 < size; ++i)
        if (data[i] == 0 && data[i+1] == 0 && data[i+2] == 1 && (data[i+3] & 31) == type) return true;
    return false;
}
extern "C" int32_t lc_ts_create(const LcTsConfig* c, LcTsMux** out) {
    if (!out) return LC_INVALID;
    *out = nullptr;
    if (!c || c->size < sizeof(LcTsConfig) || c->abi_version != LC_MEDIA_ABI || !c->write ||
        !c->width || !c->height || c->width > 1920 || c->height > 1080 || c->fps == 0 || c->fps > 60 ||
        c->audio_channels > 2 || (c->audio_channels && (c->audio_rate != 48000 || c->audio_specific_config_size != 2 || !c->audio_specific_config))) return LC_INVALID;
    try {
        auto m = std::make_unique<LcTsMux>(); m->config = *c;
        if (avformat_alloc_output_context2(&m->format, nullptr, "mpegts", nullptr) < 0) return LC_MEDIA_ERROR;
        auto* video = avformat_new_stream(m->format, nullptr); if (!video) return LC_MEDIA_ERROR;
        video->time_base = AVRational{1, 90000}; video->avg_frame_rate = AVRational{static_cast<int>(c->fps), 1};
        auto* v = video->codecpar; v->codec_type = AVMEDIA_TYPE_VIDEO; v->codec_id = AV_CODEC_ID_H264;
        v->width = static_cast<int>(c->width); v->height = static_cast<int>(c->height); v->bit_rate = c->video_bitrate;
        if (c->audio_channels) {
            auto* audio = avformat_new_stream(m->format, nullptr); if (!audio) return LC_MEDIA_ERROR;
            audio->time_base = AVRational{1, 90000}; auto* a = audio->codecpar;
            a->codec_type = AVMEDIA_TYPE_AUDIO; a->codec_id = AV_CODEC_ID_AAC; a->sample_rate = 48000;
            av_channel_layout_default(&a->ch_layout, static_cast<int>(c->audio_channels));
            a->extradata = static_cast<uint8_t*>(av_mallocz(2 + AV_INPUT_BUFFER_PADDING_SIZE));
            if (!a->extradata) return LC_MEDIA_ERROR;
            std::memcpy(a->extradata, c->audio_specific_config, 2); a->extradata_size = 2;
        }
        auto* buffer = static_cast<uint8_t*>(av_malloc(188 * 64)); if (!buffer) return LC_MEDIA_ERROR;
        m->io = avio_alloc_context(buffer, 188 * 64, 1, m.get(), nullptr, output, nullptr);
        if (!m->io) { av_free(buffer); return LC_MEDIA_ERROR; }
        m->format->pb = m->io; m->format->flags |= AVFMT_FLAG_CUSTOM_IO | AVFMT_FLAG_FLUSH_PACKETS;
        m->format->max_interleave_delta = 100000;
        AVDictionary* options = nullptr;
        av_dict_set(&options, "mpegts_flags", "resend_headers+pat_pmt_at_frames", 0);
        av_dict_set(&options, "muxdelay", "0", 0);
        int result = avformat_write_header(m->format, &options); av_dict_free(&options);
        if (result < 0) return LC_MEDIA_ERROR;
        m->header = true; *out = m.release(); return LC_OK;
    } catch (...) { return LC_MEDIA_ERROR; }
}
static int32_t write(LcTsMux* m, const uint8_t* data, size_t size, int64_t pts, bool audio) {
    if (!m || !data || !size || size > 8 * 1024 * 1024 || pts < 0) return LC_INVALID;
    if (m->failed) return LC_CLOSED;
    if (audio && !m->config.audio_channels) return LC_INVALID;
    auto& previous = audio ? m->last_audio : m->last_video;
    if (pts <= previous) return LC_INVALID;
    bool key = !audio && nal_type(data, size, 5);
    if (!audio && (!nal_type(data, size, 1) && !key)) return LC_INVALID;
    if (key && (!nal_type(data, size, 7) || !nal_type(data, size, 8))) return LC_INVALID;
    AVPacket* packet = av_packet_alloc(); if (!packet) return LC_MEDIA_ERROR;
    int result = av_new_packet(packet, static_cast<int>(size));
    if (result >= 0) {
        std::memcpy(packet->data, data, size); packet->stream_index = audio ? 1 : 0;
        packet->pts = packet->dts = av_rescale_q(pts, AVRational{1, 1000000}, m->format->streams[packet->stream_index]->time_base);
        if (key) packet->flags |= AV_PKT_FLAG_KEY;
        result = av_interleaved_write_frame(m->format, packet); avio_flush(m->io);
    }
    av_packet_free(&packet);
    if (result < 0 || m->failed) { m->failed = true; return LC_MEDIA_ERROR; }
    previous = pts; return LC_OK;
}
extern "C" int32_t lc_ts_video(LcTsMux* m, const uint8_t* d, size_t n, int64_t t) { try { return write(m,d,n,t,false); } catch (...) { return LC_MEDIA_ERROR; } }
extern "C" int32_t lc_ts_audio(LcTsMux* m, const uint8_t* d, size_t n, int64_t t) { try { return write(m,d,n,t,true); } catch (...) { return LC_MEDIA_ERROR; } }
extern "C" void lc_ts_destroy(LcTsMux* m) { try { delete m; } catch (...) {} }
