#pragma once
#include "types.h"
#include <d3d11.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <vector>
#include <wrl/client.h>

namespace lancast {
using Microsoft::WRL::ComPtr;

// Media Foundation returns a slot only after the encoder releases its sample.
// Keeping texture/view references does not claim that a sample is free.
class VideoFramePool {
  public:
    struct Frame {
        ComPtr<IMFSample> sample;
        ComPtr<ID3D11VideoProcessorOutputView> view;
    };
    VideoFramePool(ID3D11Device *device, const CaptureConfig &config);
    Frame acquire(ID3D11VideoProcessorEnumerator *enumerator);

  private:
    struct View {
        ComPtr<ID3D11Texture2D> texture;
        UINT subresource;
        ComPtr<ID3D11VideoProcessorOutputView> output;
    };
    ComPtr<IMFDXGIDeviceManager> manager_;
    ComPtr<IMFVideoSampleAllocatorEx> allocator_;
    ComPtr<ID3D11VideoDevice> video_;
    ComPtr<ID3D11VideoProcessorEnumerator> enumerator_;
    std::vector<View> views_;
};
} // namespace lancast
