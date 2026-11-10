#include "view.h"
#include <algorithm>
#include <string>

namespace {
constexpr COLORREF surfaces[] = {RGB(244, 247, 251), RGB(255, 255, 255), RGB(232, 240, 254)};
constexpr COLORREF ink = RGB(31, 45, 66), muted = RGB(88, 106, 130), accent = RGB(23, 92, 211),
                   border = RGB(215, 224, 237), disabled_ink = RGB(132, 145, 164);

class SavedDC {
  public:
    explicit SavedDC(HDC dc) : dc_(dc), state_(SaveDC(dc)) {}
    ~SavedDC() {
        if (state_)
            RestoreDC(dc_, state_);
    }
    SavedDC(const SavedDC &) = delete;
    SavedDC &operator=(const SavedDC &) = delete;

  private:
    HDC dc_;
    int state_;
};

void rounded(HDC dc, const RECT &rect, COLORREF fill, COLORREF edge, int radius) {
    SelectObject(dc, GetStockObject(DC_BRUSH));
    SelectObject(dc, GetStockObject(DC_PEN));
    SetDCBrushColor(dc, fill);
    SetDCPenColor(dc, edge);
    RoundRect(dc, rect.left, rect.top, rect.right, rect.bottom, radius, radius);
}
} // namespace

MainView::MainView() {
    for (size_t i = 0; i < std::size(brushes_); ++i)
        brushes_[i] = CreateSolidBrush(surfaces[i]);
}
MainView::~MainView() {
    for (auto font : fonts_)
        if (font)
            DeleteObject(font);
    for (auto brush : brushes_)
        DeleteObject(brush);
}

bool MainView::paint_message(UINT message, WPARAM w, LPARAM l, LRESULT &result) {
    switch (message) {
    case WM_CTLCOLORSTATIC:
    case WM_CTLCOLORBTN:
    case WM_CTLCOLOREDIT:
    case WM_CTLCOLORLISTBOX: {
        auto dc = reinterpret_cast<HDC>(w);
        auto surface = Surface::card;
        auto color = ink;
        const auto item = std::find_if(items_.begin(), items_.end(), [l](const auto &entry) {
            return entry.window == reinterpret_cast<HWND>(l);
        });
        if (item != items_.end()) {
            surface = item->surface;
            if (item->font == 3)
                color = muted;
        }
        SetTextColor(dc, color);
        SetBkColor(dc, surfaces[static_cast<size_t>(surface)]);
        result = reinterpret_cast<LRESULT>(brushes_[static_cast<size_t>(surface)]);
        return true;
    }
    case WM_ERASEBKGND:
        // Paint the backdrop and its sections together in WM_PAINT.
        result = 1;
        return true;
    case WM_PAINT: {
        PAINTSTRUCT paint{};
        auto dc = BeginPaint(window_, &paint);
        {
            SavedDC saved(dc);
            RECT client{};
            GetClientRect(window_, &client);
            FillRect(dc, &client, brushes_[static_cast<size_t>(Surface::canvas)]);
            const int extra = advanced_ ? 56 : 0;
            const auto panel = [&](int top, int bottom, Surface surface) {
                RECT rect{scale(16), scale(top) - scroll_, client.right - scale(16),
                          scale(bottom) - scroll_};
                const auto color = surfaces[static_cast<size_t>(surface)];
                rounded(dc, rect, color, surface == Surface::card ? border : color, scale(18));
            };
            panel(112, 310 + extra, Surface::card);
            panel(320 + extra, 566 + extra, Surface::card);
            panel(574 + extra, 634 + extra, Surface::notice);
            auto icon = reinterpret_cast<HICON>(SendMessageW(window_, WM_GETICON, ICON_BIG, 0));
            DrawIconEx(dc, scale(28), scale(24) - scroll_, icon, scale(36), scale(36), 0, nullptr,
                       DI_NORMAL);
        }
        EndPaint(window_, &paint);
        return true;
    }
    case WM_DRAWITEM: {
        const auto &draw = *reinterpret_cast<DRAWITEMSTRUCT *>(l);
        if (draw.CtlType != ODT_COMBOBOX && draw.CtlType != ODT_BUTTON)
            return false;
        SavedDC saved(draw.hDC);
        const bool disabled = (draw.itemState & ODS_DISABLED) != 0;
        const bool selected = (draw.itemState & ODS_SELECTED) != 0;
        SetBkMode(draw.hDC, TRANSPARENT);
        SelectObject(draw.hDC, fonts_[0]);
        auto bounds = draw.rcItem;
        if (draw.CtlType == ODT_COMBOBOX) {
            SetDCBrushColor(draw.hDC, selected && !disabled ? accent : surfaces[1]);
            FillRect(draw.hDC, &draw.rcItem, static_cast<HBRUSH>(GetStockObject(DC_BRUSH)));
            std::wstring label;
            if (draw.itemID != static_cast<UINT>(-1)) {
                const auto length = SendMessageW(draw.hwndItem, CB_GETLBTEXTLEN, draw.itemID, 0);
                if (length >= 0 && length <= 4096) {
                    label.resize(static_cast<size_t>(length) + 1);
                    SendMessageW(draw.hwndItem, CB_GETLBTEXT, draw.itemID,
                                 reinterpret_cast<LPARAM>(label.data()));
                    label.resize(static_cast<size_t>(length));
                }
            } else if (draw.CtlID == devices)
                label = L"请选择搜索到的电视";
            else if (draw.CtlID == sources)
                label = L"请选择屏幕或窗口";
            SetTextColor(draw.hDC, disabled ? disabled_ink : selected ? surfaces[1] : ink);
            bounds.left += scale(8);
            DrawTextW(draw.hDC, label.c_str(), -1, &bounds,
                      DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
        } else {
            const bool primary = draw.CtlID == 5;
            const bool connect = draw.CtlID == 4;
            const auto fill = disabled              ? RGB(241, 244, 248)
                              : primary             ? (selected ? RGB(18, 66, 157) : accent)
                              : selected || connect ? surfaces[2]
                                                    : surfaces[1];
            FillRect(draw.hDC, &draw.rcItem, brushes_[static_cast<size_t>(Surface::card)]);
            rounded(draw.hDC, draw.rcItem, fill, primary && !disabled ? fill : border, scale(10));
            wchar_t label[128]{};
            GetWindowTextW(draw.hwndItem, label, ARRAYSIZE(label));
            SetTextColor(draw.hDC, disabled  ? disabled_ink
                                   : primary ? surfaces[1]
                                   : connect ? accent
                                             : ink);
            bounds.left += scale(6);
            bounds.right -= scale(6);
            DrawTextW(draw.hDC, label, -1, &bounds,
                      DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        }
        if ((draw.itemState & (ODS_FOCUS | ODS_NOFOCUSRECT)) == ODS_FOCUS) {
            auto rect = draw.rcItem;
            InflateRect(&rect, -scale(4), -scale(4));
            DrawFocusRect(draw.hDC, &rect);
        }
        return true;
    }
    }
    return false;
}
