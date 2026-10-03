#pragma once
#include "types.h"
#include <audioclient.h>
#include <mmdeviceapi.h>
#include <wrl/client.h>
#include <vector>
namespace lancast {
class Loopback {
public:
    Loopback();
    ~Loopback();
    std::vector<int16_t> poll();
private:
    Microsoft::WRL::ComPtr<IAudioClient> client_;
    Microsoft::WRL::ComPtr<IAudioCaptureClient> capture_;
};
}
