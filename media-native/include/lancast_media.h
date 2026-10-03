#ifndef LANCAST_MEDIA_H
#define LANCAST_MEDIA_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
#define LC_MEDIA_ABI 1u
typedef struct LcTsMux LcTsMux;
/* Called synchronously; bytes valid only until return. Return nonzero on backpressure/closed. */
typedef int32_t (*LcTsWrite)(void* user, const uint8_t* bytes, size_t length);
typedef struct LcTsConfig {
    uint32_t size, abi_version;
    uint32_t width, height, fps, video_bitrate;
    uint32_t audio_rate, audio_channels; /* channels=0 disables AAC */
    const uint8_t* audio_specific_config;
    size_t audio_specific_config_size;
    LcTsWrite write;
    void* user;
} LcTsConfig;
enum { LC_OK=0, LC_INVALID=-1, LC_CLOSED=-2, LC_MEDIA_ERROR=-3 };
int32_t lc_ts_create(const LcTsConfig* config, LcTsMux** mux);
/* H.264 Annex-B AU or raw AAC AU, monotonic per-track microseconds, pts=dts (no B frames). */
int32_t lc_ts_video(LcTsMux* mux, const uint8_t* data, size_t size, int64_t pts_us);
int32_t lc_ts_audio(LcTsMux* mux, const uint8_t* data, size_t size, int64_t pts_us);
/* Not callable from write callback. Owner serializes all calls and destroy. */
void lc_ts_destroy(LcTsMux* mux);
/* H.264 Annex-B metadata normalization; encoded slices are never decoded/re-encoded. */
typedef struct LcH264Filter LcH264Filter;
typedef int32_t (*LcH264Write)(void*, const uint8_t*, size_t, int64_t pts_us, uint32_t keyframe);
typedef struct LcH264Config {
    uint32_t size, abi_version;
    LcH264Write write;
    void* user;
} LcH264Config;
int32_t lc_h264_create(const LcH264Config* config, LcH264Filter** filter);
int32_t lc_h264_write(LcH264Filter* filter, const uint8_t* bytes, size_t length, int64_t pts_us, uint32_t keyframe);
void lc_h264_destroy(LcH264Filter* filter);
#ifdef __cplusplus
}
#endif
#endif
