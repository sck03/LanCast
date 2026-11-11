#include "view.h"
#include <algorithm>
#include <commctrl.h>
#include <windowsx.h>

void MainView::set_text(int id, const std::wstring &text) {
    const auto control = get(id);
    const auto length = GetWindowTextLengthW(control);
    if (static_cast<size_t>(length) == text.size()) {
        std::wstring current(text.size() + 1, L'\0');
        GetWindowTextW(control, current.data(), static_cast<int>(current.size()));
        current.resize(text.size());
        if (current == text)
            return;
    }
    SetWindowTextW(control, text.c_str());
}

void MainView::enable(int id, bool enabled) {
    const auto control = get(id);
    if ((IsWindowEnabled(control) != FALSE) != enabled)
        EnableWindow(control, enabled);
}

void MainView::set_choices(int id, const std::vector<std::wstring> &labels, int selected) {
    const auto control = get(id);
    // WM_SETREDRAW(TRUE) also sets WS_VISIBLE: do not use it on a hidden selector.
    const bool visible = IsWindowVisible(control) != FALSE;
    if (visible)
        SendMessageW(control, WM_SETREDRAW, FALSE, 0);
    SendMessageW(control, CB_RESETCONTENT, 0, 0);
    for (const auto &label : labels)
        SendMessageW(control, CB_ADDSTRING, 0, reinterpret_cast<LPARAM>(label.c_str()));
    SendMessageW(control, CB_SETCURSEL, selected, 0);
    if (visible) {
        SendMessageW(control, WM_SETREDRAW, TRUE, 0);
        RedrawWindow(control, nullptr, nullptr, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME);
    }
}

HWND MainView::add(const wchar_t *type, const wchar_t *label, DWORD style, int x, int y, int width,
                   int height, int id, int font, bool advanced, Surface surface) {
    if (lstrcmpW(type, L"BUTTON") == 0 && (style & BS_TYPEMASK) != BS_AUTOCHECKBOX)
        style = (style & ~BS_TYPEMASK) | BS_OWNERDRAW | BS_NOTIFY;
    if (lstrcmpW(type, L"COMBOBOX") == 0)
        style |= CBS_OWNERDRAWFIXED | CBS_HASSTRINGS;
    const auto window = CreateWindowExW(0, type, label, WS_CHILD | WS_VISIBLE | style, 0, 0, 0, 0,
                                        window_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)),
                                        GetModuleHandleW(nullptr), nullptr);
    items_.push_back({window, x, y, width, height, font, advanced, surface});
    return window;
}
void MainView::create(HWND window) {
    window_ = window;
    add(L"STATIC", L"LanCast", 0, 76, 23, 220, 38, 0, 2, false, Surface::canvas);
    add(L"STATIC", L"把电脑上的精彩，分享给大屏幕", 0, 28, 76, 650, 24, 0, 3, false,
        Surface::canvas);
    add(L"STATIC", L"1   选择电视", 0, 28, 124, 300, 28, 0, 1);
    add(L"STATIC", L"正在识别本机网络…", SS_NOPREFIX, 28, 157, 780, 24, network_summary, 3);
    add(L"COMBOBOX", L"", CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP, 28, 190, 620, 260, devices);
    add(L"BUTTON", L"刷新电视", WS_TABSTOP, 668, 187, 144, 36, 1);
    add(L"STATIC", L"电视配对码", 0, 28, 238, 106, 26, 0);
    auto code = add(L"EDIT", L"", WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL | ES_NUMBER, 138, 232,
                    180, 34, invitation);
    SendMessageW(code, EM_SETCUEBANNER, 0, reinterpret_cast<LPARAM>(L"电视上的 8 位数字"));
    SendMessageW(code, EM_SETLIMITTEXT, 16, 0);
    add(L"BUTTON", L"连接电视", BS_OWNERDRAW | WS_TABSTOP, 332, 231, 142, 36, 4);
    add(L"BUTTON", L"高级设置…", WS_TABSTOP, 488, 231, 150, 36, 15);
    add(L"BUTTON", L"连接帮助", WS_TABSTOP, 652, 231, 160, 36, 12);
    add(L"STATIC", L"自动获取地址与指纹；普通电视（DLNA）选中后即可播放视频。", 0, 28, 280, 784, 24,
        0, 3);

    add(L"STATIC", L"本机网络", 0, 28, 316, 112, 24, 0, 0, true);
    add(L"COMBOBOX", L"", CBS_DROPDOWNLIST | WS_TABSTOP | WS_VSCROLL, 146, 310, 492, 180, 16, 0,
        true);
    add(L"BUTTON", L"重新自动选择", WS_TABSTOP, 652, 310, 160, 32, 17, 0, true);

    add(L"STATIC", L"2   选择分享内容", 0, 28, 328, 400, 28, 0, 1);
    add(L"STATIC", L"仅分享选中的屏幕或窗口", SS_RIGHT, 462, 331, 350, 24, 0, 3);
    add(L"COMBOBOX", L"", CBS_DROPDOWNLIST | WS_TABSTOP | WS_VSCROLL, 28, 366, 620, 260, sources);
    add(L"BUTTON", L"刷新来源", WS_TABSTOP, 668, 363, 144, 36, 13);
    auto audio_check = add(L"BUTTON", L"同时分享电脑声音（包括其他应用播放的声音）",
                           BS_AUTOCHECKBOX | WS_TABSTOP, 28, 410, 784, 28, audio);
    SendMessageW(audio_check, BM_SETCHECK, BST_CHECKED, 0);
    add(L"BUTTON", L"开始投屏", BS_OWNERDRAW | WS_TABSTOP, 28, 451, 250, 46, 5);
    add(L"BUTTON", L"播放视频文件", WS_TABSTOP, 294, 451, 250, 46, 6);
    add(L"BUTTON", L"测试电视兼容性", WS_TABSTOP, 560, 451, 252, 46, 14);
    add(L"BUTTON", L"暂停", WS_TABSTOP, 28, 520, 140, 34, 7);
    add(L"BUTTON", L"继续播放", WS_TABSTOP, 184, 520, 140, 34, 8);
    add(L"BUTTON", L"跳到 60 秒", WS_TABSTOP, 340, 520, 160, 34, 9);
    add(L"BUTTON", L"停止投屏", WS_TABSTOP, 516, 520, 140, 34, 10);
    add(L"BUTTON", L"断开连接", WS_TABSTOP, 672, 520, 140, 34, 11);
    add(L"STATIC", L"正在准备…", SS_LEFT | SS_NOPREFIX, 28, 585, 784, 44, status, 0, false,
        Surface::notice);
    add(L"STATIC", L"最小化会收到托盘并继续投屏；关闭窗口会退出。", 0, 28, 646, 784, 24, 0, 3,
        false, Surface::canvas);
    fonts();
    resize();
}
void MainView::fonts() {
    dpi_ = static_cast<int>(GetDpiForWindow(window_));
    const int sizes[] = {16, 19, 28, 14};
    for (int i = 0; i < 4; ++i) {
        if (fonts_[i])
            DeleteObject(fonts_[i]);
        fonts_[i] = CreateFontW(-scale(sizes[i]), 0, 0, 0,
                                i == 1 || i == 2 ? FW_SEMIBOLD : FW_NORMAL, FALSE, FALSE, FALSE,
                                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                                CLEARTYPE_QUALITY, DEFAULT_PITCH, L"Microsoft YaHei UI");
    }
    for (const auto &item : items_)
        SendMessageW(item.window, WM_SETFONT, reinterpret_cast<WPARAM>(fonts_[item.font]), TRUE);
    for (int id : {devices, sources, 16}) {
        SendMessageW(get(id), CB_SETITEMHEIGHT, static_cast<WPARAM>(-1), scale(28));
        SendMessageW(get(id), CB_SETITEMHEIGHT, 0, scale(26));
    }
}
void MainView::resize() {
    RECT client{};
    GetClientRect(window_, &client);
    const int total = scale(686 + (advanced_ ? 56 : 0));
    scroll_ = std::clamp(scroll_, 0, std::max(0, total - static_cast<int>(client.bottom)));
    SCROLLINFO info{sizeof(info),
                    SIF_RANGE | SIF_PAGE | SIF_POS,
                    0,
                    total - 1,
                    static_cast<UINT>(client.bottom),
                    scroll_,
                    0};
    SetScrollInfo(window_, SB_VERT, &info, TRUE);
    // The scrollbar may have appeared/disappeared, changing the available width.
    GetClientRect(window_, &client);
    const int width = std::max(720, MulDiv(client.right, 96, dpi_));
    const auto place = [&](const Item &item, HDWP batch) {
        const bool visible = !item.advanced || advanced_;
        const int y = item.y + (!item.advanced && item.y >= 328 && advanced_ ? 56 : 0);
        // Rows use proportions so long translated labels stay usable when resized.
        const int x = 28 + MulDiv(item.x - 28, width - 56, 784);
        const int w = MulDiv(item.width, width - 56, 784);
        const UINT flags =
            SWP_NOZORDER | SWP_NOACTIVATE | (visible ? SWP_SHOWWINDOW : SWP_HIDEWINDOW);
        if (batch)
            return DeferWindowPos(batch, item.window, nullptr, scale(x), scale(y) - scroll_,
                                  scale(w), scale(item.height), flags);
        SetWindowPos(item.window, nullptr, scale(x), scale(y) - scroll_, scale(w),
                     scale(item.height), flags);
        return static_cast<HDWP>(nullptr);
    };
    auto batch = BeginDeferWindowPos(static_cast<int>(items_.size()));
    for (const auto &item : items_) {
        if (!batch)
            break;
        batch = place(item, batch);
    }
    // Failure discards the entire deferred batch, including its earlier entries.
    if (!batch || !EndDeferWindowPos(batch))
        for (const auto &item : items_)
            place(item, nullptr);
    InvalidateRect(window_, nullptr, TRUE);
}
void MainView::scroll_to(int position) {
    RECT client{};
    GetClientRect(window_, &client);
    const int total = scale(686 + (advanced_ ? 56 : 0));
    position = std::clamp(position, 0, std::max(0, total - static_cast<int>(client.bottom)));
    if (position == scroll_)
        return;
    scroll_ = position;
    resize();
}
void MainView::toggle_advanced() {
    advanced_ = !advanced_;
    SetWindowTextW(get(15), advanced_ ? L"收起高级设置" : L"高级设置…");
    resize();
}
bool MainView::handle(UINT message, WPARAM w, LPARAM l, LRESULT &result) {
    result = 0;
    switch (message) {
    case WM_COMMAND: {
        const auto focus = GetFocus();
        if (focus != reinterpret_cast<HWND>(l) || !focus)
            return false;
        const auto notification = HIWORD(w);
        if (notification != EN_SETFOCUS && notification != BN_SETFOCUS &&
            notification != CBN_SETFOCUS)
            return false;
        RECT control{}, client{};
        GetWindowRect(focus, &control);
        GetClientRect(window_, &client);
        MapWindowPoints(nullptr, window_, reinterpret_cast<POINT *>(&control), 2);
        if (control.top < 0)
            scroll_to(scroll_ + control.top - scale(16));
        else if (control.bottom > client.bottom)
            scroll_to(scroll_ + control.bottom - client.bottom + scale(16));
        return false;
    }
    case WM_SIZE:
        if (w != SIZE_MINIMIZED)
            resize();
        return false;
    case WM_DPICHANGED: {
        auto *bounds = reinterpret_cast<RECT *>(l);
        fonts();
        SetWindowPos(window_, nullptr, bounds->left, bounds->top, bounds->right - bounds->left,
                     bounds->bottom - bounds->top, SWP_NOZORDER | SWP_NOACTIVATE);
        resize();
        return true;
    }
    case WM_GETMINMAXINFO: {
        auto *info = reinterpret_cast<MINMAXINFO *>(l);
        info->ptMinTrackSize = {scale(760), scale(540)};
        return true;
    }
    case WM_VSCROLL: {
        SCROLLINFO info{sizeof(info), SIF_ALL};
        GetScrollInfo(window_, SB_VERT, &info);
        int position = scroll_;
        switch (LOWORD(w)) {
        case SB_LINEUP:
            position -= scale(36);
            break;
        case SB_LINEDOWN:
            position += scale(36);
            break;
        case SB_PAGEUP:
            position -= static_cast<int>(info.nPage);
            break;
        case SB_PAGEDOWN:
            position += static_cast<int>(info.nPage);
            break;
        case SB_THUMBTRACK:
        case SB_THUMBPOSITION:
            position = info.nTrackPos;
            break;
        case SB_TOP:
            position = 0;
            break;
        case SB_BOTTOM:
            position = info.nMax;
            break;
        }
        scroll_to(position);
        return true;
    }
    case WM_MOUSEWHEEL: {
        UINT lines = 3;
        SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &lines, 0);
        RECT client{};
        GetClientRect(window_, &client);
        scroll_to(scroll_ - wheel_.consume(GET_WHEEL_DELTA_WPARAM(w), lines, scale(24),
                                           static_cast<int>(client.bottom)));
        return true;
    }
    }
    return paint_message(message, w, l, result);
}
