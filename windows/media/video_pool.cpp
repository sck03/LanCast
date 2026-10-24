#include "video_pool.h"
#include <algorithm>

namespace lancast {
VideoFramePool::VideoFramePool(ID3D11Device *device, const CaptureConfig &c) {
    UINT token = 0;
    check(MFCreateDXGIDeviceManager(&token, &manager_), "GPU_POOL_MANAGER_FAILED");
    check(manager_->ResetDevice(device, token), "GPU_POOL_DEVICE_FAILED");
    check(device->QueryInterface(IID_PPV_ARGS(&video_)), "GPU_VIDEO_UNAVAILABLE");
    check(MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator_)), "GPU_POOL_FAILED");
    check(allocator_->SetDirectXManager(manager_.Get()), "GPU_POOL_MANAGER_FAILED");
    ComPtr<IMFMediaType> type;
    check(MFCreateMediaType(&type), "GPU_POOL_TYPE_FAILED");
    check(type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video), "GPU_POOL_TYPE_FAILED");
    check(type->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_NV12), "GPU_POOL_FORMAT_FAILED");
    check(MFSetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, c.width, c.height),
          "GPU_POOL_SIZE_FAILED");
    check(MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, c.fps, 1), "GPU_POOL_RATE_FAILED");
    check(MFSetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1),
          "GPU_POOL_ASPECT_FAILED");
    check(type->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive),
          "GPU_POOL_TYPE_FAILED");
    ComPtr<IMFAttributes> attributes;
    check(MFCreateAttributes(&attributes, 2), "GPU_POOL_ATTRIBUTES_FAILED");
    check(attributes->SetUINT32(MF_SA_D3D11_BINDFLAGS, D3D11_BIND_RENDER_TARGET),
          "GPU_POOL_BIND_FAILED");
    check(attributes->SetUINT32(MF_SA_D3D11_USAGE, D3D11_USAGE_DEFAULT), "GPU_POOL_USAGE_FAILED");
    check(allocator_->InitializeSampleAllocatorEx(2, 4, attributes.Get(), type.Get()),
          "NV12_POOL_FAILED");
}
VideoFramePool::Frame VideoFramePool::acquire(ID3D11VideoProcessorEnumerator *enumerator) {
    Frame frame;
    const auto result = allocator_->AllocateSample(&frame.sample);
    if (result == MF_E_SAMPLEALLOCATOR_EMPTY)
        return frame; // Drop this video input, keeping audio and the worker responsive.
    check(result, "GPU_POOL_SAMPLE_FAILED");
    ComPtr<IMFMediaBuffer> buffer;
    ComPtr<IMFDXGIBuffer> dxgi;
    check(frame.sample->GetBufferByIndex(0, &buffer), "GPU_POOL_BUFFER_FAILED");
    check(buffer.As(&dxgi), "GPU_POOL_DXGI_FAILED");
    ComPtr<ID3D11Texture2D> texture;
    UINT subresource = 0;
    check(dxgi->GetResource(IID_PPV_ARGS(&texture)), "GPU_POOL_TEXTURE_FAILED");
    check(dxgi->GetSubresourceIndex(&subresource), "GPU_POOL_SLICE_FAILED");
    if (enumerator_.Get() != enumerator) {
        views_.clear();
        enumerator_ = enumerator;
    }
    const auto found = std::find_if(views_.begin(), views_.end(), [&](const auto &v) {
        return v.texture.Get() == texture.Get() && v.subresource == subresource;
    });
    if (found != views_.end()) {
        frame.view = found->output;
        return frame;
    }
    D3D11_TEXTURE2D_DESC texture_desc{};
    texture->GetDesc(&texture_desc);
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC desc{};
    if (texture_desc.ArraySize > 1) {
        desc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2DARRAY;
        desc.Texture2DArray.MipSlice = subresource % texture_desc.MipLevels;
        desc.Texture2DArray.FirstArraySlice = subresource / texture_desc.MipLevels;
        desc.Texture2DArray.ArraySize = 1;
    } else {
        desc.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
        desc.Texture2D.MipSlice = subresource;
    }
    check(video_->CreateVideoProcessorOutputView(texture.Get(), enumerator, &desc, &frame.view),
          "GPU_OUTPUT_VIEW_FAILED");
    if (views_.size() == 4)
        views_.erase(views_.begin());
    views_.push_back({texture, subresource, frame.view});
    return frame;
}
} // namespace lancast
