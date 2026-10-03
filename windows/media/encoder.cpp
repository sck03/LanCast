#include "encoder.h"
#include <algorithm>
#include <cstring>
#include <wmcodecdsp.h>

namespace lancast {
static ComPtr<IMFMediaType> video_type(GUID subtype, const CaptureConfig &c) {
    ComPtr<IMFMediaType> t;
    check(MFCreateMediaType(&t), "MEDIA_TYPE_FAILED");
    check(t->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video), "VIDEO_TYPE_FAILED");
    check(t->SetGUID(MF_MT_SUBTYPE, subtype), "VIDEO_SUBTYPE_FAILED");
    check(MFSetAttributeSize(t.Get(), MF_MT_FRAME_SIZE, c.width, c.height), "VIDEO_SIZE_FAILED");
    check(MFSetAttributeRatio(t.Get(), MF_MT_FRAME_RATE, c.fps, 1), "VIDEO_RATE_FAILED");
    check(MFSetAttributeRatio(t.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1), "VIDEO_ASPECT_FAILED");
    check(t->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive),
          "VIDEO_INTERLACE_FAILED");
    check(t->SetUINT32(MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709), "VIDEO_PRIMARIES_FAILED");
    check(t->SetUINT32(MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709), "VIDEO_TRANSFER_FAILED");
    check(t->SetUINT32(MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709), "VIDEO_MATRIX_FAILED");
    check(t->SetUINT32(MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235), "VIDEO_RANGE_FAILED");
    if (subtype == MFVideoFormat_H264) {
        check(t->SetUINT32(MF_MT_AVG_BITRATE, c.bitrate), "BITRATE_FAILED");
        check(t->SetUINT32(MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base), "H264_PROFILE_FAILED");
        check(t->SetUINT32(MF_MT_MPEG2_LEVEL, eAVEncH264VLevel3_1), "H264_LEVEL_FAILED");
    }
    return t;
}
void Encoder::configure_codec(const GUID &key, uint32_t value) {
    if (!codec_)
        return;
    VARIANT v;
    VariantInit(&v);
    v.vt = VT_UI4;
    v.ulVal = value;
    check(codec_->SetValue(&key, &v), "ENCODER_PARAMETER_FAILED");
}
Encoder::Encoder(ID3D11Device *device, const CaptureConfig &c, PacketSink sink)
    : sink_(std::move(sink)), kind_(Codec::H264), fps_(c.fps), bitrate_(c.bitrate) {
    MFT_REGISTER_TYPE_INFO in{MFMediaType_Video, MFVideoFormat_NV12},
        out{MFMediaType_Video, MFVideoFormat_H264};
    IMFActivate **activations = nullptr;
    UINT32 count = 0;
    check(MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER,
                    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, &in, &out, &activations,
                    &count),
          "H264_ENUM_FAILED");
    HRESULT selected = E_FAIL;
    for (UINT32 i = 0; i < count; ++i) {
        if (!transform_)
            selected = activations[i]->ActivateObject(IID_PPV_ARGS(&transform_));
        activations[i]->Release();
    }
    CoTaskMemFree(activations);
    if (!transform_)
        check(FAILED(selected) ? selected : E_FAIL, "H264_HARDWARE_UNAVAILABLE");
    ComPtr<IMFAttributes> attributes;
    check(transform_->GetAttributes(&attributes), "MFT_ATTRIBUTES_FAILED");
    UINT32 async = 0;
    attributes->GetUINT32(MF_TRANSFORM_ASYNC, &async);
    asynchronous_ = async != 0;
    if (asynchronous_) {
        check(attributes->SetUINT32(MF_TRANSFORM_ASYNC_UNLOCK, TRUE), "MFT_ASYNC_FAILED");
        check(transform_.As(&events_), "MFT_EVENTS_FAILED");
    }
    UINT32 aware = 0;
    attributes->GetUINT32(MF_SA_D3D11_AWARE, &aware);
    if (!aware)
        throw std::runtime_error("H264_GPU_INPUT_UNAVAILABLE");
    UINT token = 0;
    check(MFCreateDXGIDeviceManager(&token, &manager_), "GPU_MANAGER_FAILED");
    check(manager_->ResetDevice(device, token), "GPU_DEVICE_FAILED");
    check(transform_->ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER,
                                     reinterpret_cast<ULONG_PTR>(manager_.Get())),
          "GPU_ENCODER_FAILED");
    transform_.As(&codec_);
    configure_codec(CODECAPI_AVEncCommonRateControlMode, eAVEncCommonRateControlMode_CBR);
    configure_codec(CODECAPI_AVEncCommonMeanBitRate, c.bitrate);
    configure_codec(CODECAPI_AVEncMPVGOPSize, c.fps);
    configure_codec(CODECAPI_AVEncMPVDefaultBPictureCount, 0);
    if (codec_) {
        VARIANT v;
        VariantInit(&v);
        v.vt = VT_BOOL;
        v.boolVal = VARIANT_TRUE;
        codec_->SetValue(&CODECAPI_AVLowLatencyMode, &v);
    }
    auto output = video_type(MFVideoFormat_H264, c), input = video_type(MFVideoFormat_NV12, c);
    check(transform_->SetOutputType(0, output.Get(), 0), "H264_OUTPUT_FAILED");
    check(transform_->SetInputType(0, input.Get(), 0), "H264_INPUT_FAILED");
    check(transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0), "H264_BEGIN_FAILED");
    check(transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0), "H264_START_FAILED");
}
Encoder::Encoder(PacketSink sink) : sink_(std::move(sink)), kind_(Codec::Aac) {
    check(CoCreateInstance(CLSID_AACMFTEncoder, nullptr, CLSCTX_INPROC_SERVER,
                           IID_PPV_ARGS(&transform_)),
          "AAC_UNAVAILABLE");
    auto make = [](bool compressed) {
        ComPtr<IMFMediaType> t;
        check(MFCreateMediaType(&t), "AAC_TYPE_FAILED");
        t->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio);
        t->SetGUID(MF_MT_SUBTYPE, compressed ? MFAudioFormat_AAC : MFAudioFormat_PCM);
        t->SetUINT32(MF_MT_AUDIO_NUM_CHANNELS, 2);
        t->SetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000);
        t->SetUINT32(MF_MT_AUDIO_BITS_PER_SAMPLE, 16);
        t->SetUINT32(MF_MT_AUDIO_AVG_BYTES_PER_SECOND, compressed ? 16000 : 192000);
        if (compressed) {
            t->SetUINT32(MF_MT_AAC_PAYLOAD_TYPE, 0);
            t->SetUINT32(MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29);
        } else {
            t->SetUINT32(MF_MT_AUDIO_BLOCK_ALIGNMENT, 4);
            t->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE);
        }
        return t;
    };
    auto out = make(true), in = make(false);
    check(transform_->SetOutputType(0, out.Get(), 0), "AAC_OUTPUT_FAILED");
    check(transform_->SetInputType(0, in.Get(), 0), "AAC_INPUT_FAILED");
    check(transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0), "AAC_BEGIN_FAILED");
    check(transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0), "AAC_START_FAILED");
}
Encoder::~Encoder() {
    if (transform_)
        transform_->ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
}
void Encoder::output() {
    MFT_OUTPUT_STREAM_INFO info{};
    check(transform_->GetOutputStreamInfo(0, &info), "ENCODER_OUTPUT_INFO_FAILED");
    ComPtr<IMFSample> allocated;
    if (!(info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES)) {
        check(MFCreateSample(&allocated), "SAMPLE_FAILED");
        ComPtr<IMFMediaBuffer> buffer;
        check(MFCreateAlignedMemoryBuffer(std::max<DWORD>(info.cbSize, 4096),
                                          info.cbAlignment ? info.cbAlignment - 1 : 0, &buffer),
              "OUTPUT_BUFFER_FAILED");
        allocated->AddBuffer(buffer.Get());
    }
    MFT_OUTPUT_DATA_BUFFER out{};
    out.pSample = allocated.Get();
    DWORD status = 0;
    auto hr = transform_->ProcessOutput(0, 1, &out, &status);
    if (out.pEvents)
        out.pEvents->Release();
    ComPtr<IMFSample> provided;
    if (!allocated && out.pSample)
        provided.Attach(out.pSample);
    if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT)
        return;
    if (hr == MF_E_TRANSFORM_STREAM_CHANGE) {
        ComPtr<IMFMediaType> type;
        check(transform_->GetOutputAvailableType(0, 0, &type), "ENCODER_FORMAT_FAILED");
        check(transform_->SetOutputType(0, type.Get(), 0), "ENCODER_FORMAT_FAILED");
        return;
    }
    check(hr, "ENCODER_OUTPUT_FAILED");
    if (!out.pSample)
        return;
    ComPtr<IMFMediaBuffer> buffer;
    check(out.pSample->ConvertToContiguousBuffer(&buffer), "PACKET_FAILED");
    BYTE *data = nullptr;
    DWORD size = 0;
    check(buffer->Lock(&data, nullptr, &size), "PACKET_LOCK_FAILED");
    std::vector<uint8_t> bytes(data, data + size);
    buffer->Unlock();
    LONGLONG timestamp = 0;
    check(out.pSample->GetSampleTime(&timestamp), "PACKET_TIMESTAMP_FAILED");
    UINT32 key = 0;
    out.pSample->GetUINT32(MFSampleExtension_CleanPoint, &key);
    if (kind_ == Codec::H264 && key) {
        ComPtr<IMFMediaType> type;
        check(transform_->GetOutputCurrentType(0, &type), "H264_HEADERS_FAILED");
        UINT32 length = 0;
        if (SUCCEEDED(type->GetBlobSize(MF_MT_MPEG_SEQUENCE_HEADER, &length)) && length < 65536) {
            headers_.resize(length);
            check(type->GetBlob(MF_MT_MPEG_SEQUENCE_HEADER, headers_.data(), length, nullptr),
                  "H264_HEADERS_FAILED");
        }
        if (!headers_.empty())
            bytes.insert(bytes.begin(), headers_.begin(), headers_.end());
    }
    if (!bytes.empty())
        sink_({kind_, bytes, timestamp / 10, key != 0});
}
void Encoder::drain() {
    if (asynchronous_) {
        for (;;) {
            ComPtr<IMFMediaEvent> event;
            auto hr = events_->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event);
            if (hr == MF_E_NO_EVENTS_AVAILABLE)
                break;
            check(hr, "ENCODER_EVENT_FAILED");
            MediaEventType type;
            check(event->GetType(&type), "ENCODER_EVENT_FAILED");
            HRESULT result;
            check(event->GetStatus(&result), "ENCODER_STATUS_FAILED");
            check(result, "ENCODER_ASYNC_FAILED");
            if (type == METransformNeedInput)
                ++input_slots_;
            else if (type == METransformHaveOutput)
                output();
        }
    } else {
        // Synchronous AAC typically emits one AU per input; more output is drained before the next
        // input.
        output();
    }
}
bool Encoder::input(ComPtr<IMFSample> sample) {
    if (asynchronous_ && !input_slots_)
        return false;
    auto hr = transform_->ProcessInput(0, sample.Get(), 0);
    if (hr == MF_E_NOTACCEPTING) {
        drain();
        return false;
    }
    check(hr, "ENCODER_INPUT_FAILED");
    if (asynchronous_)
        --input_slots_;
    drain();
    return true;
}
bool Encoder::video(ID3D11Texture2D *texture, int64_t pts_us, bool keyframe, uint32_t bitrate) {
    drain();
    if (asynchronous_ && !input_slots_)
        return false;
    if (keyframe)
        configure_codec(CODECAPI_AVEncVideoForceKeyFrame, 1);
    if (bitrate != bitrate_) {
        configure_codec(CODECAPI_AVEncCommonMeanBitRate, bitrate);
        bitrate_ = bitrate;
    }
    ComPtr<IMFMediaBuffer> buffer;
    check(MFCreateDXGISurfaceBuffer(__uuidof(ID3D11Texture2D), texture, 0, FALSE, &buffer),
          "GPU_SAMPLE_FAILED");
    ComPtr<IMFSample> sample;
    check(MFCreateSample(&sample), "SAMPLE_FAILED");
    sample->AddBuffer(buffer.Get());
    sample->SetSampleTime(pts_us * 10);
    sample->SetSampleDuration(10000000 / fps_);
    return input(sample);
}
void Encoder::audio(std::span<const int16_t> pcm, int64_t pts_us) {
    ComPtr<IMFMediaBuffer> buffer;
    check(MFCreateMemoryBuffer(static_cast<DWORD>(pcm.size_bytes()), &buffer), "PCM_BUFFER_FAILED");
    BYTE *data;
    check(buffer->Lock(&data, nullptr, nullptr), "PCM_LOCK_FAILED");
    std::memcpy(data, pcm.data(), pcm.size_bytes());
    buffer->Unlock();
    buffer->SetCurrentLength(static_cast<DWORD>(pcm.size_bytes()));
    ComPtr<IMFSample> sample;
    check(MFCreateSample(&sample), "PCM_SAMPLE_FAILED");
    sample->AddBuffer(buffer.Get());
    sample->SetSampleTime(pts_us * 10);
    sample->SetSampleDuration(static_cast<LONGLONG>(pcm.size() / 2) * 10000000 / 48000);
    if (!input(sample) && !input(sample))
        throw std::runtime_error("AAC_BACKPRESSURE");
}
} // namespace lancast
