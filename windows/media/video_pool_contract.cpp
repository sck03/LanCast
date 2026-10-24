#include "video_pool.h"
#include <chrono>
#include <iostream>
#include <thread>

int main() {
    using namespace lancast;
    const auto apartment = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    bool media_started = false;
    try {
        check(apartment, "COM_FAILED");
        check(MFStartup(MF_VERSION, MFSTARTUP_LITE), "MF_START_FAILED");
        media_started = true;
        ComPtr<ID3D11Device> device;
        ComPtr<ID3D11DeviceContext> context;
        const auto hardware =
            D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
                              D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                              nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context);
        ComPtr<ID3D11VideoDevice> video;
        const auto video_support = SUCCEEDED(hardware) ? device.As(&video) : hardware;
        int result = 0;
        if (FAILED(hardware)) {
            std::cout << "SKIP: no hardware D3D11 device on this host\n";
            result = 77;
        } else if (video_support == E_NOINTERFACE) {
            std::cout << "SKIP: this host's D3D11 device has no video processing interface\n";
            result = 77;
        } else {
            check(video_support, "VIDEO_DEVICE_FAILED");
            CaptureConfig config;
            VideoFramePool pool(device.Get(), config);
            D3D11_VIDEO_PROCESSOR_CONTENT_DESC desc{};
            desc.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
            desc.InputWidth = desc.OutputWidth = config.width;
            desc.InputHeight = desc.OutputHeight = config.height;
            desc.InputFrameRate = desc.OutputFrameRate = {30, 1};
            desc.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
            ComPtr<ID3D11VideoProcessorEnumerator> enumerator;
            check(video->CreateVideoProcessorEnumerator(&desc, &enumerator), "VIDEO_ENUM_FAILED");
            std::vector<VideoFramePool::Frame> held;
            for (int i = 0; i < 4; ++i) {
                auto frame = pool.acquire(enumerator.Get());
                if (!frame.sample || !frame.view)
                    throw std::runtime_error("pool could not allocate its bounded slots");
                held.push_back(std::move(frame));
            }
            if (pool.acquire(enumerator.Get()).sample)
                throw std::runtime_error("pool reused an in-flight encoder sample");
            held.front().sample.Reset(); // Views may stay alive; they do not own a sample lease.
            const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
            VideoFramePool::Frame recycled;
            while (!recycled.sample && std::chrono::steady_clock::now() < deadline) {
                recycled = pool.acquire(enumerator.Get());
                if (!recycled.sample)
                    std::this_thread::sleep_for(std::chrono::milliseconds(1));
            }
            if (!recycled.sample)
                throw std::runtime_error("released sample was not returned to the pool");
            std::cout << "PASS: bounded NV12 pool, retained-sample isolation and reuse\n";
        }
        MFShutdown();
        CoUninitialize();
        return result;
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        if (media_started)
            MFShutdown();
        if (SUCCEEDED(apartment))
            CoUninitialize();
        return 1;
    }
}
