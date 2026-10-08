#pragma once
#include <windows.h>

#include <shellapi.h>
#include <string>

class TrayIcon {
  public:
    using Notify = decltype(&Shell_NotifyIconW);
    explicit TrayIcon(Notify notify = Shell_NotifyIconW) : notify_(notify) {}
    TrayIcon(const TrayIcon &) = delete;
    TrayIcon &operator=(const TrayIcon &) = delete;
    static constexpr UINT message = WM_APP + 2;
    static constexpr int restore = 201, stop = 202, quit = 203;
    ~TrayIcon();
    void attach(HWND window, HICON icon);
    void remove();
    bool hide();
    void show();
    void update(const std::wstring &status);
    void event(LPARAM value);
    bool recreate();
    UINT taskbar_created() const {
        return taskbar_created_;
    }

  private:
    Notify notify_;
    NOTIFYICONDATAW data_{};
    UINT taskbar_created_ = RegisterWindowMessageW(L"TaskbarCreated");
    bool added_ = false, notified_ = false;
};
