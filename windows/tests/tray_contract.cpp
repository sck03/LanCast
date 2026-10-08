#include "tray.h"
#include <iostream>
#include <stdexcept>

namespace {
bool shell_available = true, icon_present = false;

BOOL WINAPI notify(DWORD operation, NOTIFYICONDATAW *data) {
    if (!shell_available || !IsWindow(data->hWnd))
        return FALSE;
    switch (operation) {
    case NIM_ADD:
        if (icon_present)
            return FALSE;
        icon_present = true;
        return TRUE;
    case NIM_DELETE:
        icon_present = false;
        return TRUE;
    case NIM_MODIFY:
    case NIM_SETVERSION:
        return icon_present;
    default:
        return FALSE;
    }
}

void check(bool condition, const char *message) {
    if (!condition)
        throw std::runtime_error(message);
}
} // namespace

int main() {
    HWND window = nullptr;
    try {
        WNDCLASSW klass{};
        klass.lpfnWndProc = DefWindowProcW;
        klass.hInstance = GetModuleHandleW(nullptr);
        klass.lpszClassName = L"LanCastTrayContract";
        check(RegisterClassW(&klass) != 0, "register test window");
        window = CreateWindowW(klass.lpszClassName, L"LanCast tray contract", WS_OVERLAPPEDWINDOW,
                               0, 0, 400, 300, nullptr, nullptr, klass.hInstance, nullptr);
        check(window != nullptr, "create test window");
        {
            TrayIcon tray(notify);
            tray.attach(window, LoadIconW(nullptr, IDI_APPLICATION));
            check(tray.hide() && icon_present && !IsWindowVisible(window),
                  "registered icon permits hiding");
            tray.recreate();
            check(icon_present && !IsWindowVisible(window),
                  "repeated registration keeps a hidden window hidden");

            tray.show();
            icon_present = false;
            check(tray.hide() && icon_present && !IsWindowVisible(window),
                  "minimize repairs a missing icon before hiding");

            icon_present = false;
            tray.recreate();
            check(icon_present && !IsWindowVisible(window),
                  "Explorer restart restores the icon without showing the window");

            shell_available = false;
            icon_present = false;
            tray.update(L"Sharing continues");
            check(IsWindowVisible(window), "shell failure restores an accessible window");
            check(!tray.hide() && IsWindowVisible(window),
                  "unavailable notification area cannot hide the window");

            shell_available = true;
            check(tray.hide() && icon_present, "notification area can recover after failure");
            tray.event(MAKELPARAM(NIN_KEYSELECT, 1));
            check(IsWindowVisible(window), "keyboard selection restores the window");

            ShowWindow(window, SW_MAXIMIZE);
            check(IsZoomed(window), "maximize test window");
            tray.show();
            check(IsZoomed(window), "opening an already maximized window preserves its size");
            check(tray.hide(), "hide maximized window");
            tray.show();
            check(IsWindowVisible(window) && IsZoomed(window),
                  "restore hidden maximized window without losing its size");

            ShowWindow(window, SW_MINIMIZE);
            check(tray.hide(), "hide minimized window");
            tray.event(MAKELPARAM(NIN_SELECT, 1));
            check(IsWindowVisible(window) && !IsIconic(window) && IsZoomed(window),
                  "minimize and restore preserve maximized placement");
        }
        check(!icon_present, "destruction removes the notification icon");
        DestroyWindow(window);
        std::cout << "tray recovery/failure/window placement contracts passed\n";
        return 0;
    } catch (const std::exception &error) {
        if (window)
            DestroyWindow(window);
        std::cerr << error.what() << '\n';
        return 1;
    }
}
