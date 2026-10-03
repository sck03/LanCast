#include "lancast_media.h"
extern "C" {
#include <libavcodec/bsf.h>
#include <libavutil/opt.h>
}
#include <cstring>
#include <new>

struct LcH264Filter {
    AVBSFContext *context = nullptr;
    LcH264Write write = nullptr;
    void *user = nullptr;
    bool failed = false;
    ~LcH264Filter() {
        av_bsf_free(&context);
    }
};
extern "C" int32_t lc_h264_create(const LcH264Config *c, LcH264Filter **output) {
    if (!output)
        return LC_INVALID;
    *output = nullptr;
    if (!c || c->size < sizeof(*c) || c->abi_version != LC_MEDIA_ABI || !c->write)
        return LC_INVALID;
    auto filter = new (std::nothrow) LcH264Filter;
    if (!filter)
        return LC_MEDIA_ERROR;
    const auto bsf = av_bsf_get_by_name("h264_metadata");
    if (!bsf || av_bsf_alloc(bsf, &filter->context) < 0) {
        delete filter;
        return LC_MEDIA_ERROR;
    }
    filter->context->par_in->codec_id = AV_CODEC_ID_H264;
    filter->context->par_in->codec_type = AVMEDIA_TYPE_VIDEO;
    filter->context->time_base_in = {1, 1000000};
    auto options = filter->context->priv_data;
    if (av_opt_set_int(options, "colour_primaries", 1, 0) < 0 ||
        av_opt_set_int(options, "transfer_characteristics", 1, 0) < 0 ||
        av_opt_set_int(options, "matrix_coefficients", 1, 0) < 0 ||
        av_opt_set_int(options, "video_full_range_flag", 0, 0) < 0 ||
        av_bsf_init(filter->context) < 0) {
        delete filter;
        return LC_MEDIA_ERROR;
    }
    filter->write = c->write;
    filter->user = c->user;
    *output = filter;
    return LC_OK;
}
extern "C" int32_t lc_h264_write(LcH264Filter *filter, const uint8_t *bytes, size_t length,
                                 int64_t pts, uint32_t keyframe) {
    if (!filter || !bytes || !length || length > 8 * 1024 * 1024 || pts < 0)
        return LC_INVALID;
    if (filter->failed)
        return LC_CLOSED;
    AVPacket *packet = av_packet_alloc();
    if (!packet)
        return LC_MEDIA_ERROR;
    int result = av_new_packet(packet, static_cast<int>(length));
    if (result >= 0) {
        std::memcpy(packet->data, bytes, length);
        packet->pts = pts;
        packet->dts = pts;
        if (keyframe)
            packet->flags |= AV_PKT_FLAG_KEY;
        result = av_bsf_send_packet(filter->context, packet);
    }
    while (result >= 0) {
        result = av_bsf_receive_packet(filter->context, packet);
        if (result == AVERROR(EAGAIN) || result == AVERROR_EOF) {
            result = 0;
            break;
        }
        if (result < 0)
            break;
        const auto accepted =
            filter->write(filter->user, packet->data, static_cast<size_t>(packet->size),
                          packet->pts, (packet->flags & AV_PKT_FLAG_KEY) != 0);
        av_packet_unref(packet);
        if (accepted != 0) {
            result = AVERROR_EXTERNAL;
            break;
        }
    }
    av_packet_free(&packet);
    if (result < 0) {
        filter->failed = true;
        return LC_MEDIA_ERROR;
    }
    return LC_OK;
}
extern "C" void lc_h264_destroy(LcH264Filter *filter) {
    delete filter;
}
