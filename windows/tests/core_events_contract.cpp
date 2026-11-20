#include "core_events.h"
#include <stdexcept>
#include <chrono>
#include <cstring>
#include <deque>

namespace {
void require(bool condition) { if (!condition) throw std::runtime_error("core event contract failed"); }
std::mutex lock;
std::condition_variable wake;
std::deque<std::string> source;
bool closed = false, fail_read = false;
unsigned consumed = 0;
LancastBuffer pop() {
    if (source.empty()) return {};
    const auto text = source.front(); source.pop_front(); ++consumed;
    auto *data = new uint8_t[text.size()];
    std::memcpy(data, text.data(), text.size());
    return {data, text.size()};
}
MSG next(HWND window) {
    const auto deadline = GetTickCount64() + 3000;
    MSG message{};
    while (GetTickCount64() < deadline) {
        if (PeekMessageW(&message, window, WM_APP, WM_APP, PM_REMOVE)) return message;
        MsgWaitForMultipleObjects(0, nullptr, FALSE, 20, QS_POSTMESSAGE);
    }
    throw std::runtime_error("event delivery timed out");
}
}
extern "C" LancastBuffer lancast_poll(LancastHandle) { std::lock_guard guard(lock); return pop(); }
extern "C" LancastBuffer lancast_wait_poll(LancastHandle, uint32_t ms) {
    std::unique_lock guard(lock);
    if (fail_read) throw std::runtime_error("synthetic native failure");
    wake.wait_for(guard, std::chrono::milliseconds(ms), [] { return closed || !source.empty(); });
    return pop();
}
extern "C" void lancast_free_buffer(LancastBuffer buffer) { delete[] buffer.data; }
extern "C" int32_t lancast_shutdown(LancastHandle) {
    { std::lock_guard guard(lock); closed = true; }
    wake.notify_all(); return 0;
}
int main() {
    const auto window = CreateWindowW(L"STATIC", L"events", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr, nullptr, nullptr);
    require(window);
    for (int i = 0; i < 96; ++i) source.push_back(std::to_string(i));
    CoreEvents events;
    events.start(window, WM_APP, 7);
    require(next(window).wParam == 7);
    { std::lock_guard guard(lock); require(consumed == 32); }
    const auto first = events.take();
    require(first.size() == 32 && first.front() == "0" && first.back() == "31");
    require(next(window).wParam == 7);
    { std::lock_guard guard(lock); require(consumed == 64); }
    // Stop must wake the worker even when the UI has not consumed its pending batch.
    events.stop();
    require(events.take().empty());
    {
        std::lock_guard guard(lock);
        source.clear(); closed = false;
    }
    events.start(window, WM_APP, 8);
    // A worker blocked in native wait is also woken and joined by stop.
    events.stop();
    { std::lock_guard guard(lock); closed = false; fail_read = true; }
    events.start(window, WM_APP, 9);
    require(next(window).wParam == 9);
    const auto failure = events.take();
    require(failure.size() == 1 && failure.front().find("\"terminal\":true") != std::string::npos);
    events.stop();
    DestroyWindow(window);
}
