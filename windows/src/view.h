#pragma once
#include <vector>
#include <windows.h>

class MainView {
  public:
    MainView() = default;
    MainView(const MainView &) = delete;
    MainView &operator=(const MainView &) = delete;
    static constexpr int devices = 100, invitation = 104, sources = 105, audio = 106, status = 107,
                         network_summary = 108;
    ~MainView();
    void create(HWND window);
    HWND get(int id) const {
        return GetDlgItem(window_, id);
    }
    void resize();
    void toggle_advanced();
    bool handle(UINT message, WPARAM w, LPARAM l, LRESULT &result);

  private:
    struct Item {
        HWND window;
        int x, y, width, height, font;
        bool advanced;
    };
    HWND window_ = nullptr;
    std::vector<Item> items_;
    HFONT fonts_[3]{};
    HBRUSH background_ = CreateSolidBrush(RGB(245, 247, 251));
    HBRUSH white_ = CreateSolidBrush(RGB(255, 255, 255));
    bool advanced_ = false;
    int dpi_ = 96, scroll_ = 0;
    HWND add(const wchar_t *type, const wchar_t *label, DWORD style, int x, int y, int width,
             int height, int id, int font = 0, bool advanced = false);
    void fonts();
    int scale(int value) const {
        return MulDiv(value, dpi_, 96);
    }
};
