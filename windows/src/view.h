#pragma once
#include "view_scroll.h"
#include <vector>
#include <windows.h>

class MainView {
  public:
    MainView();
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
    enum class Surface { canvas, card, notice };
    struct Item {
        HWND window;
        int x, y, width, height, font;
        bool advanced;
        Surface surface;
    };
    HWND window_ = nullptr;
    std::vector<Item> items_;
    HFONT fonts_[4]{};
    HBRUSH brushes_[3]{};
    bool advanced_ = false;
    int dpi_ = 96, scroll_ = 0;
    WheelScroll wheel_;
    HWND add(const wchar_t *type, const wchar_t *label, DWORD style, int x, int y, int width,
             int height, int id, int font = 0, bool advanced = false,
             Surface surface = Surface::card);
    void fonts();
    void scroll_to(int position);
    bool paint_message(UINT message, WPARAM w, LPARAM l, LRESULT &result);
    int scale(int value) const {
        return MulDiv(value, dpi_, 96);
    }
};
