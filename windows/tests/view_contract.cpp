#include "tray.h"
#include "view.h"
#include <commctrl.h>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <vector>

static MainView *view = nullptr;
static unsigned timer_ticks = 0;
static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM w, LPARAM l) {
    if (message == WM_TIMER) { ++timer_ticks; return 0; }
    LRESULT result{};
    if (view && view->handle(message, w, l, result))
        return result;
    return DefWindowProcW(window, message, w, l);
}
static void check(bool condition, const char *message) {
    if (!condition)
        throw std::runtime_error(message);
}
static void snapshot(HWND window, const char *path) {
    RECT rect{};
    GetClientRect(window, &rect);
    BITMAPINFO info{};
    info.bmiHeader = {
        sizeof(BITMAPINFOHEADER), rect.right, -rect.bottom, 1, 32, BI_RGB, 0, 0, 0, 0, 0};
    auto dc = GetDC(window);
    auto target = CreateCompatibleDC(dc);
    void *bits{};
    auto bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &bits, nullptr, 0);
    auto previous = SelectObject(target, bitmap);
    PrintWindow(window, target, PW_CLIENTONLY);
    const auto length = static_cast<DWORD>(rect.right * rect.bottom * 4);
    BITMAPFILEHEADER file{
        0x4d42, static_cast<DWORD>(sizeof(BITMAPFILEHEADER) + sizeof(BITMAPINFOHEADER)) + length, 0,
        0, sizeof(BITMAPFILEHEADER) + sizeof(BITMAPINFOHEADER)};
    std::ofstream output(path, std::ios::binary);
    output.write(reinterpret_cast<const char *>(&file), sizeof(file));
    output.write(reinterpret_cast<const char *>(&info.bmiHeader), sizeof(info.bmiHeader));
    output.write(static_cast<const char *>(bits), length);
    SelectObject(target, previous);
    DeleteObject(bitmap);
    DeleteDC(target);
    ReleaseDC(window, dc);
}
int main(int argc, char **argv) {
    try {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        INITCOMMONCONTROLSEX common{sizeof(common), ICC_STANDARD_CLASSES};
        InitCommonControlsEx(&common);
        auto instance = GetModuleHandleW(nullptr);
        WNDCLASSW klass{};
        klass.lpfnWndProc = window_proc;
        klass.hInstance = instance;
        klass.lpszClassName = L"LanCastViewContract";
        RegisterClassW(&klass);
        auto window = CreateWindowW(klass.lpszClassName, L"LanCast view contract",
                                    WS_OVERLAPPEDWINDOW | WS_VSCROLL, 0, 0, 880, 740, nullptr,
                                    nullptr, instance, nullptr);
        check(window != nullptr, "window creation");
        auto icon = LoadIconW(instance, MAKEINTRESOURCEW(1));
        check(icon != nullptr, "embedded application icon");
        SendMessageW(window, WM_SETICON, ICON_BIG, reinterpret_cast<LPARAM>(icon));
        MainView controls;
        controls.create(window);
        view = &controls;
        SetWindowTextW(controls.get(MainView::network_summary),
                       L"已自动选择网络 · 192.168.1.10（界面测试示例）");
        SetWindowTextW(controls.get(MainView::status), L"请选择电视，输入配对码后即可连接。");
        check((GetWindowLongPtrW(controls.get(MainView::address), GWL_STYLE) & WS_VISIBLE) == 0,
              "manual fields collapsed initially");
        check(SendMessageW(controls.get(MainView::audio), BM_GETCHECK, 0, 0) == BST_CHECKED,
              "audio choice initialized");
        if (argc > 1) {
            ShowWindow(window, SW_SHOWNOACTIVATE);
            UpdateWindow(window);
            snapshot(window, argv[1]);
            ShowWindow(window, SW_HIDE);
        }
        controls.toggle_advanced();
        check((GetWindowLongPtrW(controls.get(MainView::address), GWL_STYLE) & WS_VISIBLE) != 0,
              "advanced fields can be reached");
        RECT source{}, manual{};
        GetWindowRect(controls.get(MainView::sources), &source);
        GetWindowRect(controls.get(MainView::fingerprint), &manual);
        check(source.top > manual.bottom, "advanced fields do not overlap sharing controls");
        SetWindowPos(window, nullptr, 0, 0, 780, 560, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        SendMessageW(window, WM_VSCROLL, SB_BOTTOM, 0);
        SCROLLINFO scroll{sizeof(scroll), SIF_ALL};
        GetScrollInfo(window, SB_VERT, &scroll);
        check(scroll.nMax > static_cast<int>(scroll.nPage),
              "small windows retain a scrollable form");
        controls.toggle_advanced();
        check((GetWindowLongPtrW(controls.get(MainView::fingerprint), GWL_STYLE) & WS_VISIBLE) == 0,
              "collapse hides identity editor");
        {
            TrayIcon tray;
            tray.attach(window, icon);
            if (tray.hide()) {
                check(!IsWindowVisible(window), "tray hide keeps the window alive");
                SetTimer(window, 9, 10, nullptr);
                const auto deadline = GetTickCount64() + 2000;
                while (!timer_ticks && GetTickCount64() < deadline) {
                    MSG message{};
                    while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) DispatchMessageW(&message);
                    Sleep(1);
                }
                KillTimer(window, 9);
                check(timer_ticks > 0, "hidden window continues processing timers");
                tray.event(MAKELPARAM(NIN_SELECT, 1));
                check(IsWindowVisible(window), "tray selection restores window");
                tray.remove();
                tray.recreate();
                check(tray.hide(), "icon can be registered again after removal");
                std::cout << "tray hide/timer/restore/re-registration passed\n";
            } else {
                std::cout << "SKIP: notification area unavailable on this test host\n";
            }
        }
        view = nullptr;
        DestroyWindow(window);
        std::cout << "desktop view/resource contracts passed\n";
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
