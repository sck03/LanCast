#include "h264_metadata.h"
namespace lancast {
H264Metadata::H264Metadata(PacketSink output) : output_(std::move(output)) {
    wchar_t buffer[32768];
    DWORD n = GetModuleFileNameW(nullptr, buffer, 32768);
    if (!n || n >= 32768)
        throw std::runtime_error("APPLICATION_PATH_FAILED");
    std::wstring path(buffer, n);
    path.resize(path.find_last_of(L"\\/") + 1);
    path += L"lancast_ts.dll";
    module_ = LoadLibraryExW(path.c_str(), nullptr,
                             LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!module_)
        throw std::runtime_error("MEDIA_METADATA_LIBRARY_MISSING");
    auto create =
        reinterpret_cast<decltype(&lc_h264_create)>(GetProcAddress(module_, "lc_h264_create"));
    write_ = reinterpret_cast<decltype(write_)>(GetProcAddress(module_, "lc_h264_write"));
    destroy_ = reinterpret_cast<decltype(destroy_)>(GetProcAddress(module_, "lc_h264_destroy"));
    LcH264Config c{};
    c.size = sizeof(c);
    c.abi_version = LC_MEDIA_ABI;
    c.user = this;
    c.write = [](void *user, const uint8_t *bytes, size_t size, int64_t pts,
                 uint32_t key) -> int32_t {
        auto &self = *static_cast<H264Metadata *>(user);
        try {
            self.output_({Codec::H264, std::span(bytes, size), pts, key != 0});
            return 0;
        } catch (...) {
            self.output_error_ = std::current_exception();
            return -1;
        }
    };
    if (!create || !write_ || !destroy_ || create(&c, &filter_) != LC_OK) {
        FreeLibrary(module_);
        module_ = nullptr;
        throw std::runtime_error("H264_METADATA_INITIALIZATION_FAILED");
    }
}
H264Metadata::~H264Metadata() {
    if (filter_)
        destroy_(filter_);
    if (module_)
        FreeLibrary(module_);
}
void H264Metadata::send(const Packet &p) {
    output_error_ = nullptr;
    if (write_(filter_, p.bytes.data(), p.bytes.size(), p.time_us, p.keyframe) != LC_OK) {
        if (output_error_)
            std::rethrow_exception(output_error_);
        throw std::runtime_error("H264_METADATA_FAILED");
    }
}
} // namespace lancast
