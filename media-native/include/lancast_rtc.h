#ifndef LANCAST_RTC_H
#define LANCAST_RTC_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Implemented by the source-built platform media library, not by the control core. */
typedef uint64_t LcRtcHandle;
typedef void (*LcRtcEvent)(void* user, const uint8_t* json, size_t length);
/* Events may arrive on media threads. Queue a copy; do not reenter this ABI in a callback. */
typedef struct LcRtcConfig {
    uint32_t size, abi_version;
    uint64_t window_handle;
    uint32_t width, height, fps, bitrate, audio;
    LcRtcEvent event;
    void* user;
    /* ABI 2: route 0=WebRTC, 1=DLNA TS. synthetic never captures the desktop or audio. */
    uint32_t route, synthetic;
    int32_t (*write_ts)(void* user, const uint8_t* bytes, size_t length);
    void* ts_user;
    uint64_t monitor_handle;
} LcRtcConfig;
typedef uint32_t (*LcRtcVersionFn)(void);
typedef LcRtcHandle (*LcRtcCreateFn)(const LcRtcConfig*);
typedef int32_t (*LcRtcCommandFn)(LcRtcHandle, const uint8_t*, size_t);
/* Shutdown must join all callbacks before it returns. Called off the UI thread. */
typedef void (*LcRtcDestroyFn)(LcRtcHandle);
#ifdef __cplusplus
}
#endif
#endif
