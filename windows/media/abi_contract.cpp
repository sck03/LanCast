#include "lancast_rtc.h"
#include <atomic>
#include <chrono>
#include <iostream>
#include <thread>
#include <windows.h>

int wmain(int argc, wchar_t **argv) {
    if (argc != 2)
        return 1;
    // Deliberately repeat DLL load/create/stop/unload. Synthetic mode cannot capture the desktop.
    // Encoding can report hardware unavailable on a headless runner; unloading must still be safe.
    for (int attempt = 0; attempt < 3; ++attempt) {
        HMODULE module = LoadLibraryExW(
            argv[1], nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!module) {
            std::cerr << "media DLL load failed: " << GetLastError();
            return 1;
        }
        auto version = reinterpret_cast<LcRtcVersionFn>(GetProcAddress(module, "lc_rtc_version"));
        auto create = reinterpret_cast<LcRtcCreateFn>(GetProcAddress(module, "lc_rtc_create"));
        auto command = reinterpret_cast<LcRtcCommandFn>(GetProcAddress(module, "lc_rtc_command"));
        auto destroy = reinterpret_cast<LcRtcDestroyFn>(GetProcAddress(module, "lc_rtc_destroy"));
        if (!version || version() != 2 || !create || !command || !destroy || create(nullptr))
            return 1;
        std::atomic_int callbacks{0};
        LcRtcConfig c{};
        c.size = sizeof(c);
        c.abi_version = 2;
        c.width = 640;
        c.height = 360;
        c.fps = 30;
        c.bitrate = 1000000;
        c.synthetic = 1;
        c.user = &callbacks;
        c.event = [](void *p, const uint8_t *, size_t) { ++*static_cast<std::atomic_int *>(p); };
        auto handle = create(&c);
        if (!handle)
            return 1;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        destroy(handle);
        const auto count = callbacks.load();
        destroy(handle);
        const uint8_t invalid[]{'{', '}'};
        if (command(handle, invalid, sizeof(invalid)) != -2)
            return 1;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        if (callbacks != count) {
            std::cerr << "late callback after destroy";
            return 1;
        }
        FreeLibrary(module);
    }
    std::cout << "PASS: repeated synthetic media DLL lifecycle, invalid handles and no callbacks "
                 "after destroy\n";
    return 0;
}
