#include "lancast_rtc.h"
#include <atomic>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <thread>
#include <windows.h>

// Developer diagnostic: synthetic-only, no window/monitor/audio capture handles are created.
struct Output {
    std::ofstream file;
    std::atomic_bool failed{false};
    std::atomic_size_t bytes{0};
};
int wmain(int argc, wchar_t **argv) {
    if (argc != 3 || std::filesystem::exists(argv[2])) {
        std::cerr << "usage: media_probe absolute-lancast_rtc.dll new-output.ts\n";
        return 1;
    }
    HMODULE module = LoadLibraryExW(
        argv[1], nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!module) {
        std::cerr << "DLL load failed: " << GetLastError();
        return 1;
    }
    auto create = reinterpret_cast<LcRtcCreateFn>(GetProcAddress(module, "lc_rtc_create"));
    auto destroy = reinterpret_cast<LcRtcDestroyFn>(GetProcAddress(module, "lc_rtc_destroy"));
    if (!create || !destroy) {
        FreeLibrary(module);
        return 1;
    }
    Output out;
    out.file.open(std::filesystem::path(argv[2]), std::ios::binary);
    if (!out.file) {
        FreeLibrary(module);
        return 1;
    }
    LcRtcConfig c{};
    c.size = sizeof(c);
    c.abi_version = 2;
    c.width = 1280;
    c.height = 720;
    c.fps = 30;
    c.bitrate = 3000000;
    c.audio = 1;
    c.synthetic = 1;
    c.route = 1;
    c.user = &out;
    c.ts_user = &out;
    c.event = [](void *p, const uint8_t *bytes, size_t length) {
        std::cerr.write(reinterpret_cast<const char *>(bytes),
                        static_cast<std::streamsize>(length));
        std::cerr << '\n';
        static_cast<Output *>(p)->failed = true;
    };
    c.write_ts = [](void *p, const uint8_t *bytes, size_t length) -> int32_t {
        auto &output = *static_cast<Output *>(p);
        output.file.write(reinterpret_cast<const char *>(bytes),
                          static_cast<std::streamsize>(length));
        output.bytes += length;
        return output.file ? 0 : -1;
    };
    const auto handle = create(&c);
    if (!handle) {
        FreeLibrary(module);
        std::cerr << "Synthetic media initialization failed\n";
        return 2;
    }
    const auto end = std::chrono::steady_clock::now() + std::chrono::seconds(6);
    while (!out.failed && std::chrono::steady_clock::now() < end)
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    destroy(handle);
    out.file.close();
    FreeLibrary(module);
    if (out.failed || out.bytes < 188 * 20)
        return 2;
    std::cout << "Synthetic hardware H.264 + AAC -> TS bytes=" << out.bytes
              << "; decode the output before treating this as media success.\n";
    return 0;
}
