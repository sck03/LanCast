#include "audio.h"
#include <cstring>
namespace lancast {
Loopback::Loopback() {
    check(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                           IID_PPV_ARGS(&enumerator_)),
          "AUDIO_ENUM_FAILED");
    Microsoft::WRL::ComPtr<IMMDevice> device;
    check(enumerator_->GetDefaultAudioEndpoint(eRender, eConsole, &device),
          "AUDIO_OUTPUT_UNAVAILABLE");
    LPWSTR id = nullptr;
    check(device->GetId(&id), "AUDIO_DEVICE_ID_FAILED");
    device_id_ = id;
    CoTaskMemFree(id);
    check(device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr,
                           reinterpret_cast<void **>(client_.GetAddressOf())),
          "AUDIO_CLIENT_FAILED");
    WAVEFORMATEX format{WAVE_FORMAT_PCM, 2, 48000, 192000, 4, 16, 0};
    check(client_->Initialize(AUDCLNT_SHAREMODE_SHARED,
                              AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
                                  AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                              200000, 0, &format, nullptr),
          "SYSTEM_AUDIO_FORMAT_UNAVAILABLE");
    check(client_->GetService(IID_PPV_ARGS(&capture_)), "LOOPBACK_FAILED");
    check(client_->Start(), "LOOPBACK_START_FAILED");
}
Loopback::~Loopback() {
    if (client_)
        client_->Stop();
}
std::vector<int16_t> Loopback::poll() {
    if (clock_us() >= next_device_check_) {
        next_device_check_ = clock_us() + 1000000;
        Microsoft::WRL::ComPtr<IMMDevice> device;
        check(enumerator_->GetDefaultAudioEndpoint(eRender, eConsole, &device),
              "AUDIO_OUTPUT_UNAVAILABLE");
        LPWSTR id = nullptr;
        check(device->GetId(&id), "AUDIO_DEVICE_ID_FAILED");
        const bool changed = device_id_ != id;
        CoTaskMemFree(id);
        if (changed)
            throw std::runtime_error("AUDIO_DEVICE_CHANGED_RESTART_REQUIRED");
    }
    std::vector<int16_t> result;
    UINT32 frames = 0;
    check(capture_->GetNextPacketSize(&frames), "AUDIO_DEVICE_CHANGED_RESTART_REQUIRED");
    while (frames) {
        BYTE *data = nullptr;
        DWORD flags = 0;
        check(capture_->GetBuffer(&data, &frames, &flags, nullptr, nullptr),
              "LOOPBACK_READ_FAILED");
        const auto offset = result.size();
        result.resize(offset + static_cast<size_t>(frames) * 2);
        if (!(flags & AUDCLNT_BUFFERFLAGS_SILENT))
            std::memcpy(result.data() + offset, data, static_cast<size_t>(frames) * 4);
        check(capture_->ReleaseBuffer(frames), "LOOPBACK_RELEASE_FAILED");
        if (result.size() > 19200)
            throw std::runtime_error("AUDIO_BACKPRESSURE");
        check(capture_->GetNextPacketSize(&frames), "LOOPBACK_READ_FAILED");
    }
    return result;
}
} // namespace lancast
