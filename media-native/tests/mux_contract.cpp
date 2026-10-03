#include "lancast_media.h"
#include <cstdlib>
#include <vector>
#include <cstdio>
static int32_t receive(void* u, const uint8_t* p, size_t n) {
    if (n % 188 || n > 65424) std::abort();
    auto& bytes = *static_cast<std::vector<uint8_t>*>(u); bytes.insert(bytes.end(), p, p+n); return 0;
}
int main() {
    std::vector<uint8_t> bytes;
    LcTsConfig c{}; c.size = sizeof(c); c.abi_version = 1; c.width = 1280; c.height = 720; c.fps = 30; c.video_bitrate = 3000000; c.write = receive; c.user = &bytes;
    LcTsMux* mux = nullptr;
    if (lc_ts_create(&c, &mux) != LC_OK) return 1;
    // Framing contract only, not a decodable H.264 sample or visual test.
    const uint8_t au[] = {0,0,0,1,0x67,0x42,0,0x1f,0x80,0,0,0,1,0x68,0x80,0,0,0,1,0x65,0x88,0x80};
    if (lc_ts_video(mux, au, sizeof(au), 0) != LC_OK) return 2;
    if (lc_ts_video(mux, au, sizeof(au), 0) != LC_INVALID) return 3;
    lc_ts_destroy(mux);
    if (bytes.empty() || bytes.size() % 188) return 4;
    for (size_t i=0; i<bytes.size(); i+=188) if (bytes[i] != 0x47) return 5;
    std::puts("PASS: TS framing, callback bound, timestamps, lifecycle (not decoder validation)");
}
