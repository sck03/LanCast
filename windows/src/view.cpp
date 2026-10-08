#include "view.h"
#include <algorithm>
#include <commctrl.h>
#include <string>
#include <windowsx.h>

MainView::~MainView() {
    for (auto font : fonts_)
        if (font)
            DeleteObject(font);
    DeleteObject(background_);
    DeleteObject(white_);
}
HWND MainView::add(const wchar_t *type, const wchar_t *label, DWORD style, int x, int y, int width,
                   int height, int id, int font, bool advanced) {
    if (lstrcmpW(type, L"BUTTON") == 0 && (style & BS_TYPEMASK) != BS_AUTOCHECKBOX)
        style = (style & ~BS_TYPEMASK) | BS_OWNERDRAW | BS_NOTIFY;
    if (lstrcmpW(type, L"COMBOBOX") == 0)
        style |= CBS_OWNERDRAWFIXED | CBS_HASSTRINGS;
    const auto window = CreateWindowExW(0, type, label, WS_CHILD | WS_VISIBLE | style, 0, 0, 0, 0,
                                        window_, reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)),
                                        GetModuleHandleW(nullptr), nullptr);
    items_.push_back({window, x, y, width, height, font, advanced});
    return window;
}
void MainView::create(HWND window) {
    window_ = window;
    add(L"STATIC", L"LanCast", 0, 76, 23, 220, 38, 0, 2);
    add(L"STATIC", L"把电脑上的精彩，分享给大屏幕", 0, 28, 76, 650, 24, 0);
    add(L"STATIC", L"1   选择电视", 0, 28, 124, 300, 28, 0, 1);
    add(L"STATIC", L"正在识别本机网络…", 0, 28, 157, 780, 24, network_summary);
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
        0);

    add(L"STATIC", L"本机网络", 0, 28, 316, 112, 24, 0, 0, true);
    add(L"COMBOBOX", L"", CBS_DROPDOWNLIST | WS_TABSTOP | WS_VSCROLL, 146, 310, 492, 180, 16, 0,
        true);
    add(L"BUTTON", L"重新自动选择", WS_TABSTOP, 652, 310, 160, 32, 17, 0, true);

    add(L"STATIC", L"2   选择分享内容", 0, 28, 328, 400, 28, 0, 1);
    add(L"COMBOBOX", L"", CBS_DROPDOWNLIST | WS_TABSTOP | WS_VSCROLL, 28, 366, 620, 260, sources);
    add(L"BUTTON", L"刷新窗口", WS_TABSTOP, 668, 363, 144, 36, 13);
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
    add(L"STATIC", L"正在准备…", SS_LEFT | SS_NOPREFIX, 28, 579, 784, 52, status);
    add(L"STATIC", L"最小化会收到托盘并继续投屏；关闭窗口会退出。", 0, 28, 642, 784, 24, 0);
    fonts();
    resize();
}
void MainView::fonts() {
    dpi_ = static_cast<int>(GetDpiForWindow(window_));
    const int sizes[] = {16, 19, 28};
    for (int i = 0; i < 3; ++i) {
        if (fonts_[i])
            DeleteObject(fonts_[i]);
        fonts_[i] =
            CreateFontW(-scale(sizes[i]), 0, 0, 0, i ? FW_SEMIBOLD : FW_NORMAL, FALSE, FALSE, FALSE,
                        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                        DEFAULT_PITCH, L"Microsoft YaHei UI");
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
    int width = std::max(720, MulDiv(client.right, 96, dpi_));
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
    for (const auto &item : items_) {
        ShowWindow(item.window, item.advanced && !advanced_ ? SW_HIDE : SW_SHOW);
        const int y = item.y + (!item.advanced && item.y >= 328 && advanced_ ? 56 : 0);
        // Rows use proportions so long translated labels stay usable when resized.
        const int x = 28 + MulDiv(item.x - 28, width - 56, 784);
        const int w = MulDiv(item.width, width - 56, 784);
        MoveWindow(item.window, scale(x), scale(y) - scroll_, scale(w), scale(item.height), TRUE);
    }
    InvalidateRect(window_, nullptr, TRUE);
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
            scroll_ += control.top - scale(16);
        else if (control.bottom > client.bottom)
            scroll_ += control.bottom - client.bottom + scale(16);
        else
            return false;
        resize();
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
        switch (LOWORD(w)) {
        case SB_LINEUP:
            scroll_ -= scale(36);
            break;
        case SB_LINEDOWN:
            scroll_ += scale(36);
            break;
        case SB_PAGEUP:
            scroll_ -= static_cast<int>(info.nPage);
            break;
        case SB_PAGEDOWN:
            scroll_ += static_cast<int>(info.nPage);
            break;
        case SB_THUMBTRACK:
            scroll_ = info.nTrackPos;
            break;
        case SB_TOP:
            scroll_ = 0;
            break;
        case SB_BOTTOM:
            scroll_ = info.nMax;
            break;
        }
        resize();
        return true;
    }
    case WM_MOUSEWHEEL:
        scroll_ -= GET_WHEEL_DELTA_WPARAM(w) / WHEEL_DELTA * scale(72);
        resize();
        return true;
    case WM_CTLCOLORSTATIC:
    case WM_CTLCOLORBTN: {
        auto dc = reinterpret_cast<HDC>(w);
        SetTextColor(dc, RGB(36, 51, 73));
        SetBkColor(dc, RGB(245, 247, 251));
        result = reinterpret_cast<LRESULT>(background_);
        return true;
    }
    case WM_CTLCOLOREDIT:
    case WM_CTLCOLORLISTBOX: {
        auto dc = reinterpret_cast<HDC>(w);
        SetTextColor(dc, RGB(25, 40, 60));
        SetBkColor(dc, RGB(255, 255, 255));
        result = reinterpret_cast<LRESULT>(white_);
        return true;
    }
    case WM_ERASEBKGND: {
        RECT r{};
        GetClientRect(window_, &r);
        FillRect(reinterpret_cast<HDC>(w), &r, background_);
        result = 1;
        return true;
    }
    case WM_PAINT: {
        PAINTSTRUCT paint{};
        auto dc = BeginPaint(window_, &paint);
        auto icon = reinterpret_cast<HICON>(SendMessageW(window_, WM_GETICON, ICON_BIG, 0));
        DrawIconEx(dc, scale(28), scale(24) - scroll_, icon, scale(36), scale(36), 0, nullptr,
                   DI_NORMAL);
        EndPaint(window_, &paint);
        return true;
    }
    case WM_DRAWITEM: {
        auto *draw = reinterpret_cast<DRAWITEMSTRUCT *>(l);
        if (draw->CtlType == ODT_COMBOBOX) {
            const bool selected = (draw->itemState & ODS_SELECTED) != 0;
            auto brush = CreateSolidBrush(selected ? RGB(23, 92, 211) : RGB(255, 255, 255));
            FillRect(draw->hDC, &draw->rcItem, brush);
            DeleteObject(brush);
            std::wstring label;
            if (draw->itemID != static_cast<UINT>(-1)) {
                const auto length = SendMessageW(draw->hwndItem, CB_GETLBTEXTLEN, draw->itemID, 0);
                if (length >= 0 && length <= 4096) {
                    label.resize(static_cast<size_t>(length) + 1);
                    SendMessageW(draw->hwndItem, CB_GETLBTEXT, draw->itemID,
                                 reinterpret_cast<LPARAM>(label.data()));
                    label.resize(static_cast<size_t>(length));
                }
            } else if (draw->CtlID == devices)
                label = L"请选择搜索到的电视";
            SetBkMode(draw->hDC, TRANSPARENT);
            SetTextColor(draw->hDC, selected                           ? RGB(255, 255, 255)
                                    : (draw->itemState & ODS_DISABLED) ? RGB(147, 159, 178)
                                                                       : RGB(36, 51, 73));
            SelectObject(draw->hDC, fonts_[0]);
            auto bounds = draw->rcItem;
            bounds.left += scale(8);
            DrawTextW(draw->hDC, label.c_str(), -1, &bounds,
                      DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
            if (draw->itemState & ODS_FOCUS)
                DrawFocusRect(draw->hDC, &draw->rcItem);
            return true;
        }
        if (draw->CtlType != ODT_BUTTON)
            return false;
        bool disabled = (draw->itemState & ODS_DISABLED) != 0;
        const bool primary = draw->CtlID == 4 || draw->CtlID == 5;
        const bool pressed = (draw->itemState & ODS_SELECTED) != 0;
        auto color = primary ? (disabled  ? RGB(175, 187, 206)
                                : pressed ? RGB(18, 66, 157)
                                          : RGB(23, 92, 211))
                             : (pressed ? RGB(230, 237, 248) : RGB(255, 255, 255));
        auto brush = CreateSolidBrush(color);
        auto old = SelectObject(draw->hDC, brush);
        auto border = CreatePen(PS_SOLID, 1, primary ? color : RGB(210, 219, 232));
        auto pen = SelectObject(draw->hDC, border);
        RoundRect(draw->hDC, draw->rcItem.left, draw->rcItem.top, draw->rcItem.right,
                  draw->rcItem.bottom, scale(10), scale(10));
        SelectObject(draw->hDC, old);
        SelectObject(draw->hDC, pen);
        DeleteObject(border);
        DeleteObject(brush);
        wchar_t label[128]{};
        GetWindowTextW(draw->hwndItem, label, 128);
        SetBkMode(draw->hDC, TRANSPARENT);
        SetTextColor(draw->hDC, primary    ? RGB(255, 255, 255)
                                : disabled ? RGB(147, 159, 178)
                                           : RGB(36, 51, 73));
        SelectObject(draw->hDC, fonts_[0]);
        DrawTextW(draw->hDC, label, -1, &draw->rcItem, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        if (draw->itemState & ODS_FOCUS) {
            auto rect = draw->rcItem;
            InflateRect(&rect, -4, -4);
            DrawFocusRect(draw->hDC, &rect);
        }
        return true;
    }
    }
    return false;
}
