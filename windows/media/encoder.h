#pragma once
#include "types.h"
#include <d3d11.h>
#include <mfapi.h>
#include <mfidl.h>
#include <mftransform.h>
#include <codecapi.h>
#include <wrl/client.h>
#include <vector>
namespace lancast {
using Microsoft::WRL::ComPtr;
// All calls are serialized by the capture worker. Async MFT events are polled on that worker.
class Encoder {
public:
    Encoder(ID3D11Device* device, const CaptureConfig& config, PacketSink sink);
    Encoder(PacketSink sink); // AAC-LC, PCM16 stereo 48 kHz
    ~Encoder();
    bool video(ID3D11Texture2D* nv12, int64_t pts_us, bool keyframe, uint32_t bitrate);
    void audio(std::span<const int16_t> pcm, int64_t pts_us);
    void drain();
private:
    void output();
    bool input(ComPtr<IMFSample> sample);
    void configure_codec(const GUID& key, uint32_t value);
    ComPtr<IMFTransform> transform_;
    ComPtr<IMFMediaEventGenerator> events_;
    ComPtr<IMFDXGIDeviceManager> manager_;
    ComPtr<ICodecAPI> codec_;
    PacketSink sink_;
    Codec kind_;
    bool asynchronous_ = false;
    unsigned input_slots_ = 0;
    uint32_t fps_ = 30, bitrate_ = 0;
    std::vector<uint8_t> headers_;
};
}
