#include "media.h"
#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <iostream>
#include <mutex>
#include <thread>
#include <vector>

using Clock = std::chrono::steady_clock;
using namespace std::chrono_literals;
int main() {
    std::mutex mutex;
    std::condition_variable ready;
    std::vector<std::string> events;
    auto event = [&](std::string type, const MediaSender::Json &) {
        std::lock_guard lock(mutex);
        events.push_back(std::move(type));
        ready.notify_all();
    };
    auto wait_for = [&](const std::string &type) {
        std::unique_lock lock(mutex);
        return ready.wait_for(lock, 3s, [&] {
            return std::find(events.begin(), events.end(), type) != events.end();
        });
    };
    auto responsive = [](Clock::time_point start) { return Clock::now() - start < 100ms; };
    if (!MediaSender::available())
        return 1;
    {
        MediaSender sender(event);
        auto start = Clock::now();
        sender.start(nullptr, false, MediaSender::Json::object());
        if (!responsive(start) || !wait_for("media.started"))
            return 1;
        start = Clock::now();
        sender.answer("synthetic", "synthetic");
        if (!responsive(start) || !wait_for("mock.command"))
            return 1;
        start = Clock::now();
        sender.stop();
        sender.stop();
        if (!responsive(start) || !wait_for("media.stopped") || Clock::now() - start < 150ms)
            return 1;
    }
    {
        std::lock_guard lock(mutex);
        if (std::count(events.begin(), events.end(), "media.stopped") != 1 ||
            std::find(events.begin(), events.end(), "media.error") != events.end())
            return 1;
        events.clear();
    }
    {
        MediaSender sender(event);
        sender.start(nullptr, false, MediaSender::Json::object());
        if (!wait_for("mock.creating"))
            return 1;
        sender.stop(); // Cancel after native creation has entered, before it has returned.
        if (!wait_for("media.stopped"))
            return 1;
    }
    std::this_thread::sleep_for(250ms);
    {
        std::lock_guard lock(mutex);
        if (std::count(events.begin(), events.end(), "media.stopped") != 1 ||
            events.back() != "media.stopped")
            return 1;
    }
    std::cout << "PASS: asynchronous create/command/stop, cancellation and actual media exit\n";
}
