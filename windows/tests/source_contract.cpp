#include "source_catalog.h"
#include <iostream>
#include <stdexcept>

static void check(bool value, const char *message) {
    if (!value)
        throw std::runtime_error(message);
}
static int find(const SourceCatalog &catalog, HWND window) {
    for (size_t i = 0; i < catalog.entries().size(); ++i)
        if (catalog.entries()[i].window == window)
            return static_cast<int>(i);
    return -1;
}
int main() {
    try {
        const auto instance = GetModuleHandleW(nullptr);
        WNDCLASSW klass{};
        klass.lpfnWndProc = DefWindowProcW;
        klass.hInstance = instance;
        klass.lpszClassName = L"LanCastSourceContract";
        RegisterClassW(&klass);
        const auto create = [&](const wchar_t *title, HWND owner = nullptr, DWORD extended = 0) {
            auto window = CreateWindowExW(extended, klass.lpszClassName, title, WS_OVERLAPPEDWINDOW,
                                          0, 0, 200, 100, owner, nullptr, instance, nullptr);
            check(window != nullptr, "create source test window");
            ShowWindow(window, SW_SHOWNOACTIVATE);
            return window;
        };
        const auto excluded = create(L"Sender");
        const auto popup = create(L"Sender dialog", excluded);
        check(GetWindow(popup, GW_OWNER) == excluded, "test dialog has sender owner");
        const auto first = create(L"Same title");
        const auto second = create(L"Same title");
        const auto tool = create(L"Tool", nullptr, WS_EX_TOOLWINDOW);
        SourceCatalog sources;
        int selected = sources.refresh(excluded, -1);
        check(sources.at(selected) && sources.at(selected)->available(),
              "initial source is usable");
        const auto initial = *sources.at(selected);
        if (initial.monitor) {
            auto missing_display = initial;
            missing_display.display = L"missing-display";
            check(!missing_display.available() && !initial.same_target(missing_display),
                  "a monitor handle with a different display identity is rejected");
            selected = sources.refresh(excluded, selected);
            check(sources.at(selected) && sources.at(selected)->same_target(initial),
                  "monitor selection survives a refresh");
        }
        check(find(sources, excluded) < 0, "sender window is excluded");
        check(find(sources, popup) < 0, "owned sender dialog is excluded");
        check(find(sources, tool) < 0, "tool window is excluded");
        selected = find(sources, first);
        check(selected >= 0 && find(sources, second) >= 0, "same-title windows remain distinct");
        const auto saved = *sources.at(selected);
        SetWindowTextW(first, L"Renamed window");
        SetWindowPos(second, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        selected = sources.refresh(excluded, selected);
        check(sources.at(selected) && sources.at(selected)->window == first &&
                  sources.at(selected)->name == L"Renamed window",
              "refresh preserves the target through title and order changes");
        auto wrong_process = saved;
        wrong_process.process = 0;
        check(!wrong_process.available() && !saved.same_target(wrong_process),
              "a recycled handle from another process is rejected");
        ShowWindow(first, SW_HIDE);
        check(!saved.available(), "hidden source cannot begin capture");
        ShowWindow(first, SW_SHOWNOACTIVATE);
        DestroyWindow(first);
        check(!saved.available(), "closed source cannot begin capture");
        selected = sources.refresh(excluded, selected);
        check(selected == -1 && !sources.at(selected),
              "removed source never falls back to desktop");
        check(sources.refresh(excluded, -1) == -1,
              "empty selection persists on subsequent refresh");
        check(!sources.at(1000000) && !CaptureSource{}.available(),
              "invalid selections are rejected");
        DestroyWindow(second);
        DestroyWindow(tool);
        DestroyWindow(popup);
        DestroyWindow(excluded);
        std::cout << "capture source identity/refresh/lifecycle contracts passed\n";
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
