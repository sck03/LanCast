#include "ts_output.h"
namespace lancast {
TsOutput::TsOutput(const CaptureConfig& c, LcTsWrite write, void* user) {
    wchar_t buffer[32768]; DWORD n = GetModuleFileNameW(nullptr, buffer, 32768);
    if (!n || n >= 32768) throw std::runtime_error("APPLICATION_PATH_FAILED");
    std::wstring path(buffer, n); path.resize(path.find_last_of(L"\\/") + 1); path += L"lancast_ts.dll";
    module_ = LoadLibraryExW(path.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!module_) throw std::runtime_error("TS_LIBRARY_MISSING");
    auto create = reinterpret_cast<decltype(&lc_ts_create)>(GetProcAddress(module_, "lc_ts_create"));
    video_ = reinterpret_cast<decltype(video_)>(GetProcAddress(module_, "lc_ts_video"));
    audio_ = reinterpret_cast<decltype(audio_)>(GetProcAddress(module_, "lc_ts_audio"));
    destroy_ = reinterpret_cast<decltype(destroy_)>(GetProcAddress(module_, "lc_ts_destroy"));
    uint8_t asc[]{0x11, 0x90}; LcTsConfig config{}; config.size = sizeof(config); config.abi_version = LC_MEDIA_ABI;
    config.width = c.width; config.height = c.height; config.fps = c.fps; config.video_bitrate = c.bitrate;
    config.audio_rate = 48000; config.audio_channels = c.audio ? 2 : 0; config.audio_specific_config = asc; config.audio_specific_config_size = c.audio ? 2 : 0;
    config.write = write; config.user = user;
    if (!create || !video_ || !audio_ || !destroy_ || create(&config, &mux_) != LC_OK) { FreeLibrary(module_); module_ = nullptr; throw std::runtime_error("TS_INITIALIZATION_FAILED"); }
}
TsOutput::~TsOutput() { if (mux_) destroy_(mux_); if (module_) FreeLibrary(module_); }
void TsOutput::send(const Packet& p) {
    auto result = p.codec == Codec::H264 ? video_(mux_, p.bytes.data(), p.bytes.size(), p.time_us) : audio_(mux_, p.bytes.data(), p.bytes.size(), p.time_us);
    if (result != LC_OK) throw std::runtime_error("TS_OUTPUT_FAILED");
}
}
