#pragma once
#include "types.h"
#include <audioclient.h>
#include <mmdeviceapi.h>
#include <vector>
#include <wrl/client.h>
namespace lancast {
class Loopback {
  public:
    Loopback();
    ~Loopback();
    std::vector<int16_t> poll();

  private:
    Microsoft::WRL::ComPtr<IMMDeviceEnumerator> enumerator_;
    std::wstring device_id_;
    int64_t next_device_check_ = 0;
    Microsoft::WRL::ComPtr<IAudioClient> client_;
    Microsoft::WRL::ComPtr<IAudioCaptureClient> capture_;
};
} // namespace lancast
