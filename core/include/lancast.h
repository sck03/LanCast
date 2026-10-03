#ifndef LANCAST_H
#define LANCAST_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef uint64_t LancastHandle;
typedef struct LancastBuffer { uint8_t* data; size_t len; } LancastBuffer;
LancastHandle lancast_create(void);
uint32_t lancast_abi_version(void);
/* Nonblocking. Continue polling until shutdown.completed, then destroy off the UI thread. */
int32_t lancast_shutdown(LancastHandle handle);
/* Sender only; <=65424 bytes, multiple of 188. Copies once; never waits for a TV. */
int32_t lancast_write_ts(LancastHandle handle, const uint8_t* data, size_t len);
int32_t lancast_command(LancastHandle handle, const uint8_t* utf8, size_t len);
/* Poll on the platform control thread. The returned allocation belongs to Rust. */
LancastBuffer lancast_poll(LancastHandle handle);
void lancast_free_buffer(LancastBuffer buffer);
void lancast_destroy(LancastHandle handle);
#ifdef __cplusplus
}
#endif
#endif
