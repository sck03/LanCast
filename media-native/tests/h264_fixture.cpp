#include "lancast_media.h"
#include <fstream>
#include <iostream>
#include <iterator>
#include <vector>
int main(int argc, char **argv) {
    if (argc != 3)
        return 1;
    std::ifstream input(argv[1], std::ios::binary);
    std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
    std::ofstream output(argv[2], std::ios::binary);
    if (bytes.empty() || !output)
        return 1;
    LcH264Config c{};
    c.size = sizeof(c);
    c.abi_version = LC_MEDIA_ABI;
    c.user = &output;
    c.write = [](void *user, const uint8_t *data, size_t size, int64_t pts,
                 uint32_t key) -> int32_t {
        if (pts != 0 || key != 1)
            return -1;
        auto &stream = *static_cast<std::ofstream *>(user);
        stream.write(reinterpret_cast<const char *>(data), static_cast<std::streamsize>(size));
        return stream ? 0 : -1;
    };
    LcH264Filter *filter = nullptr;
    if (lc_h264_create(&c, &filter) != LC_OK)
        return 1;
    auto result = lc_h264_write(filter, bytes.data(), bytes.size(), 0, 1);
    lc_h264_destroy(filter);
    if (result != LC_OK)
        return 1;
    std::cout << "Filtered real H.264 metadata; encoded slices are copied\n";
    return 0;
}
