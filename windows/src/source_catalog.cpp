#include "source_catalog.h"
#include <algorithm>
#include <optional>

bool CaptureSource::same_target(const CaptureSource &other) const {
    return window == other.window && monitor == other.monitor && process == other.process &&
           display == other.display;
}

bool CaptureSource::available() const {
    if (window) {
        DWORD current = 0;
        return IsWindowVisible(window) && GetWindowThreadProcessId(window, &current) &&
               current == process;
    }
    MONITORINFOEXW info{};
    info.cbSize = sizeof(info);
    return monitor && GetMonitorInfoW(monitor, &info) && display == info.szDevice;
}

const CaptureSource *SourceCatalog::at(int index) const {
    return index >= 0 && static_cast<size_t>(index) < entries_.size() ? &entries_[index] : nullptr;
}

int SourceCatalog::refresh(HWND excluded, int selected) {
    const auto *previous = at(selected);
    const auto selection = previous ? std::optional(*previous) : std::nullopt;
    std::vector<CaptureSource> next;
    EnumDisplayMonitors(
        nullptr, nullptr,
        +[](HMONITOR monitor, HDC, LPRECT, LPARAM context) -> BOOL {
            auto &entries = *reinterpret_cast<std::vector<CaptureSource> *>(context);
            MONITORINFOEXW info{};
            info.cbSize = sizeof(info);
            if (!GetMonitorInfoW(monitor, &info))
                return TRUE;
            const bool primary = (info.dwFlags & MONITORINFOF_PRIMARY) != 0;
            CaptureSource source{nullptr, monitor, 0,
                                 primary ? L"整个主屏幕"
                                         : L"扩展屏幕 · " + std::wstring(info.szDevice),
                                 info.szDevice};
            entries.insert(primary ? entries.begin() : entries.end(), std::move(source));
            return TRUE;
        },
        reinterpret_cast<LPARAM>(&next));
    struct Context {
        HWND excluded;
        std::vector<CaptureSource> &entries;
    } context{excluded, next};
    EnumWindows(
        +[](HWND window, LPARAM data) -> BOOL {
            auto &context = *reinterpret_cast<Context *>(data);
            if (window == context.excluded || !IsWindowVisible(window) ||
                (GetWindowLongPtrW(window, GWL_EXSTYLE) & WS_EX_TOOLWINDOW))
                return TRUE;
            for (auto owner = GetWindow(window, GW_OWNER); owner;
                 owner = GetWindow(owner, GW_OWNER))
                if (owner == context.excluded)
                    return TRUE;
            wchar_t title[256]{};
            if (!GetWindowTextW(window, title, ARRAYSIZE(title)))
                return TRUE;
            DWORD process = 0;
            if (GetWindowThreadProcessId(window, &process))
                context.entries.push_back({window, nullptr, process, title, {}});
            return TRUE;
        },
        reinterpret_cast<LPARAM>(&context));
    entries_ = std::move(next);
    const bool first = !initialized_;
    initialized_ = true;
    if (first)
        return entries_.empty() ? -1 : 0;
    if (selection) {
        const auto found = std::find_if(entries_.begin(), entries_.end(), [&](const auto &source) {
            return source.same_target(*selection);
        });
        if (found != entries_.end())
            return static_cast<int>(found - entries_.begin());
    }
    // A removed window must never silently turn into whole-screen sharing.
    return -1;
}
