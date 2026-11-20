#pragma once
#include "lancast.h"
#include <atomic>
#include <condition_variable>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#include <windows.h>

// One blocking native waiter and one bounded delivery batch. The UI owns consumption.
class CoreEvents {
  public:
    ~CoreEvents() { stop(); }
    void start(HWND window, UINT message, LancastHandle handle) {
        if (handle_ == handle) return;
        stop();
        if (!handle) return;
        handle_ = handle;
        stopped_ = false; failed_ = false;
        worker_ = std::thread([this, window, message, handle] {
            try {
            while (!stopped_) {
                std::vector<std::string> batch;
                auto buffer = lancast_wait_poll(handle, 1000);
                if (!buffer.data) continue;
                for (unsigned n = 0; buffer.data && n < 32; ++n) {
                    struct ReleaseBuffer {
                        LancastBuffer value;
                        ~ReleaseBuffer() { lancast_free_buffer(value); }
                    } release{buffer};
                    batch.emplace_back(reinterpret_cast<const char *>(buffer.data), buffer.len);
                    if (n < 31) buffer = lancast_poll(handle);
                }
                std::unique_lock lock(mutex_);
                if (stopped_) break;
                pending_ = std::move(batch);
                if (!PostMessageW(window, message, static_cast<WPARAM>(handle), 0)) break;
                delivered_.wait(lock, [this] { return stopped_ || pending_.empty(); });
            }
            } catch (...) {
                failed_ = true;
                PostMessageW(window, message, static_cast<WPARAM>(handle), 0);
            }
        });
    }
    std::vector<std::string> take() {
        std::lock_guard lock(mutex_);
        if (failed_) throw std::runtime_error("CONTROL_EVENT_DELIVERY_FAILED");
        auto result = std::move(pending_);
        pending_.clear();
        delivered_.notify_one();
        return result;
    }
    void stop() {
        { std::lock_guard lock(mutex_); stopped_ = true; }
        if (handle_) lancast_shutdown(handle_);
        delivered_.notify_all();
        if (worker_.joinable()) worker_.join();
        handle_ = 0;
        pending_.clear();
    }
  private:
    LancastHandle handle_ = 0;
    std::atomic_bool stopped_{true}, failed_{false};
    std::thread worker_;
    std::mutex mutex_;
    std::condition_variable delivered_;
    std::vector<std::string> pending_;
};
