#include "tray.h"
#include <algorithm>

TrayIcon::~TrayIcon() {
    remove();
}
void TrayIcon::attach(HWND window, HICON icon) {
    data_.cbSize = sizeof(data_);
    data_.hWnd = window;
    data_.uID = 1;
    data_.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    data_.uCallbackMessage = message;
    data_.hIcon = icon;
    lstrcpynW(data_.szTip, L"LanCast · 就绪", ARRAYSIZE(data_.szTip));
    recreate();
}
void TrayIcon::remove() {
    if (added_)
        Shell_NotifyIconW(NIM_DELETE, &data_);
    added_ = false;
}
void TrayIcon::recreate() {
    added_ = Shell_NotifyIconW(NIM_ADD, &data_) != FALSE;
    if (added_) {
        data_.uVersion = NOTIFYICON_VERSION_4;
        Shell_NotifyIconW(NIM_SETVERSION, &data_);
    } else if (data_.hWnd && !IsWindowVisible(data_.hWnd))
        show();
}
bool TrayIcon::hide() {
    if (!added_)
        recreate();
    if (!added_)
        return false;
    ShowWindow(data_.hWnd, SW_HIDE);
    if (!notified_) {
        data_.uFlags |= NIF_INFO;
        lstrcpynW(data_.szInfoTitle, L"LanCast 仍在运行", ARRAYSIZE(data_.szInfoTitle));
        lstrcpynW(data_.szInfo, L"投屏会继续。点击托盘图标可恢复，右键可停止投屏或退出。",
                  ARRAYSIZE(data_.szInfo));
        data_.dwInfoFlags = NIIF_INFO;
        Shell_NotifyIconW(NIM_MODIFY, &data_);
        data_.uFlags &= ~NIF_INFO;
        notified_ = true;
    }
    return true;
}
void TrayIcon::show() {
    ShowWindow(data_.hWnd, SW_RESTORE);
    SetForegroundWindow(data_.hWnd);
}
void TrayIcon::update(const std::wstring &status) {
    lstrcpynW(data_.szTip, (L"LanCast · " + status).c_str(), ARRAYSIZE(data_.szTip));
    if (added_)
        Shell_NotifyIconW(NIM_MODIFY, &data_);
}
void TrayIcon::event(LPARAM value) {
    const auto action = LOWORD(value);
    if (action == NIN_SELECT || action == NIN_KEYSELECT || action == WM_LBUTTONDBLCLK)
        show();
    else if (action == WM_CONTEXTMENU || action == WM_RBUTTONUP) {
        HMENU menu = CreatePopupMenu();
        AppendMenuW(menu, MF_STRING, restore, L"打开 LanCast");
        AppendMenuW(menu, MF_STRING, stop, L"停止投屏");
        AppendMenuW(menu, MF_SEPARATOR, 0, nullptr);
        AppendMenuW(menu, MF_STRING, quit, L"退出 LanCast");
        POINT point{};
        GetCursorPos(&point);
        SetForegroundWindow(data_.hWnd);
        const auto command = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, point.x, point.y,
                                            0, data_.hWnd, nullptr);
        if (command)
            PostMessageW(data_.hWnd, WM_COMMAND, command, 0);
        PostMessageW(data_.hWnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}
