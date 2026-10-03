#include "capture.h"
#include "audio.h"
#include "encoder.h"
#include <algorithm>
#include <cmath>
#include <d3d11_4.h>
#include <opus.h>
#include <thread>
#include <windows.graphics.capture.interop.h>
#include <windows.graphics.directx.direct3d11.interop.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.Graphics.Capture.h>
#include <winrt/Windows.Graphics.DirectX.Direct3D11.h>

namespace lancast {
using namespace winrt::Windows::Graphics;
using namespace winrt::Windows::Graphics::Capture;
using namespace winrt::Windows::Graphics::DirectX;
struct Apartment {
    Apartment() {
        winrt::init_apartment(winrt::apartment_type::multi_threaded);
        check(MFStartup(MF_VERSION, MFSTARTUP_LITE), "MEDIA_FEATURE_PACK_REQUIRED");
    }
    ~Apartment() {
        MFShutdown();
        winrt::uninit_apartment();
    }
};
class Converter {
  public:
    Converter(ID3D11Device *device, ID3D11DeviceContext *context, const CaptureConfig &c)
        : device_(device), config_(c) {
        check(device->QueryInterface(IID_PPV_ARGS(&video_)), "GPU_VIDEO_UNAVAILABLE");
        check(context->QueryInterface(IID_PPV_ARGS(&context_)), "GPU_VIDEO_CONTEXT_FAILED");
    }
    ComPtr<ID3D11Texture2D> convert(ID3D11Texture2D *input, unsigned width, unsigned height) {
        D3D11_TEXTURE2D_DESC source{};
        input->GetDesc(&source);
        width = std::min(width, source.Width);
        height = std::min(height, source.Height);
        if (!processor_ || input_width_ != source.Width || input_height_ != source.Height) {
            enumerator_.Reset();
            processor_.Reset();
            input_width_ = source.Width;
            input_height_ = source.Height;
            D3D11_VIDEO_PROCESSOR_CONTENT_DESC desc{};
            desc.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
            desc.InputWidth = source.Width;
            desc.InputHeight = source.Height;
            desc.OutputWidth = config_.width;
            desc.OutputHeight = config_.height;
            desc.InputFrameRate = {config_.fps, 1};
            desc.OutputFrameRate = {config_.fps, 1};
            desc.Usage = D3D11_VIDEO_USAGE_PLAYBACK_NORMAL;
            check(video_->CreateVideoProcessorEnumerator(&desc, &enumerator_),
                  "GPU_CONVERTER_UNAVAILABLE");
            check(video_->CreateVideoProcessor(enumerator_.Get(), 0, &processor_),
                  "GPU_CONVERTER_FAILED");
        }
        D3D11_TEXTURE2D_DESC desc{};
        desc.Width = config_.width;
        desc.Height = config_.height;
        desc.MipLevels = 1;
        desc.ArraySize = 1;
        desc.Format = DXGI_FORMAT_NV12;
        desc.SampleDesc.Count = 1;
        desc.Usage = D3D11_USAGE_DEFAULT;
        desc.BindFlags = D3D11_BIND_RENDER_TARGET;
        ComPtr<ID3D11Texture2D> output;
        check(device_->CreateTexture2D(&desc, nullptr, &output), "NV12_TEXTURE_FAILED");
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC iv{};
        iv.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
        ComPtr<ID3D11VideoProcessorInputView> input_view;
        check(video_->CreateVideoProcessorInputView(input, enumerator_.Get(), &iv, &input_view),
              "GPU_INPUT_VIEW_FAILED");
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC ov{};
        ov.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
        ComPtr<ID3D11VideoProcessorOutputView> output_view;
        check(video_->CreateVideoProcessorOutputView(output.Get(), enumerator_.Get(), &ov,
                                                     &output_view),
              "GPU_OUTPUT_VIEW_FAILED");
        const double scale =
            std::min(double(config_.width) / width, double(config_.height) / height);
        const LONG w = static_cast<LONG>(width * scale) / 2 * 2,
                   h = static_cast<LONG>(height * scale) / 2 * 2;
        RECT src{0, 0, static_cast<LONG>(width), static_cast<LONG>(height)};
        RECT dst{(static_cast<LONG>(config_.width) - w) / 2,
                 (static_cast<LONG>(config_.height) - h) / 2, 0, 0};
        dst.right = dst.left + w;
        dst.bottom = dst.top + h;
        context_->VideoProcessorSetStreamSourceRect(processor_.Get(), 0, TRUE, &src);
        context_->VideoProcessorSetStreamDestRect(processor_.Get(), 0, TRUE, &dst);
        D3D11_VIDEO_COLOR black{};
        black.RGBA.A = 1;
        context_->VideoProcessorSetOutputBackgroundColor(processor_.Get(), FALSE, &black);
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE rgb{}, yuv{};
        rgb.RGB_Range = 0;
        yuv.YCbCr_Matrix = 1;
        yuv.Nominal_Range = 1;
        context_->VideoProcessorSetStreamColorSpace(processor_.Get(), 0, &rgb);
        context_->VideoProcessorSetOutputColorSpace(processor_.Get(), &yuv);
        context_->VideoProcessorSetStreamFrameFormat(processor_.Get(), 0,
                                                     D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        D3D11_VIDEO_PROCESSOR_STREAM stream{};
        stream.Enable = TRUE;
        stream.pInputSurface = input_view.Get();
        check(context_->VideoProcessorBlt(processor_.Get(), output_view.Get(), 0, 1, &stream),
              "GPU_CONVERSION_FAILED");
        return output;
    }

  private:
    ComPtr<ID3D11Device> device_;
    CaptureConfig config_;
    unsigned input_width_ = 0, input_height_ = 0;
    ComPtr<ID3D11VideoDevice> video_;
    ComPtr<ID3D11VideoContext> context_;
    ComPtr<ID3D11VideoProcessorEnumerator> enumerator_;
    ComPtr<ID3D11VideoProcessor> processor_;
};
struct Capture::State {
    CaptureConfig config;
    PacketSink sink;
    Failure failure;
    std::atomic_bool stopped{false}, keyframe{true};
    std::atomic_uint bitrate;
    std::thread worker;
    State(CaptureConfig c, PacketSink s, Failure f)
        : config(c), sink(std::move(s)), failure(std::move(f)), bitrate(c.bitrate) {}
    void run() {
        try {
            Apartment apartment;
            ComPtr<ID3D11Device> device;
            ComPtr<ID3D11DeviceContext> context;
            check(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr,
                                    D3D11_CREATE_DEVICE_BGRA_SUPPORT |
                                        D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                                    nullptr, 0, D3D11_SDK_VERSION, &device, nullptr, &context),
                  "D3D11_HARDWARE_UNAVAILABLE");
            ComPtr<ID3D11Multithread> protection;
            check(context.As(&protection), "GPU_MULTITHREAD_FAILED");
            protection->SetMultithreadProtected(TRUE);
            Encoder video(device.Get(), config, sink);
            Converter converter(device.Get(), context.Get(), config);
            GraphicsCaptureItem item{nullptr};
            Direct3D11CaptureFramePool pool{nullptr};
            GraphicsCaptureSession session{nullptr};
            ComPtr<ID3D11Texture2D> synthetic;
            ComPtr<ID3D11RenderTargetView> canvas;
            if (config.synthetic) {
                D3D11_TEXTURE2D_DESC desc{};
                desc.Width = config.width;
                desc.Height = config.height;
                desc.MipLevels = 1;
                desc.ArraySize = 1;
                desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
                desc.SampleDesc.Count = 1;
                desc.BindFlags = D3D11_BIND_RENDER_TARGET;
                check(device->CreateTexture2D(&desc, nullptr, &synthetic), "PROBE_TEXTURE_FAILED");
                check(device->CreateRenderTargetView(synthetic.Get(), nullptr, &canvas),
                      "PROBE_VIEW_FAILED");
            } else {
                if (!GraphicsCaptureSession::IsSupported())
                    throw std::runtime_error("WGC_UNAVAILABLE");
                auto factory = winrt::get_activation_factory<GraphicsCaptureItem,
                                                             IGraphicsCaptureItemInterop>();
                if (config.window)
                    check(factory->CreateForWindow(config.window,
                                                   winrt::guid_of<GraphicsCaptureItem>(),
                                                   winrt::put_abi(item)),
                          "WINDOW_CAPTURE_DENIED");
                else {
                    auto monitor = config.monitor
                                       ? config.monitor
                                       : MonitorFromPoint({0, 0}, MONITOR_DEFAULTTOPRIMARY);
                    check(factory->CreateForMonitor(monitor, winrt::guid_of<GraphicsCaptureItem>(),
                                                    winrt::put_abi(item)),
                          "DISPLAY_CAPTURE_DENIED");
                }
                ComPtr<IDXGIDevice> dxgi;
                check(device.As(&dxgi), "DXGI_FAILED");
                winrt::com_ptr<IInspectable> inspectable;
                check(CreateDirect3D11DeviceFromDXGIDevice(dxgi.Get(), inspectable.put()),
                      "WGC_DEVICE_FAILED");
                auto direct = inspectable.as<Direct3D11::IDirect3DDevice>();
                pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                    direct, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, item.Size());
                session = pool.CreateCaptureSession(item);
                session.StartCapture();
            }
            // Locals release the session on both success and exceptions; no detached capture
            // callbacks.
            struct CloseCapture {
                Direct3D11CaptureFramePool &pool;
                GraphicsCaptureSession &session;
                ~CloseCapture() {
                    if (session)
                        session.Close();
                    if (pool)
                        pool.Close();
                }
            } close{pool, session};
            std::unique_ptr<Loopback> loopback;
            std::unique_ptr<Encoder> aac;
            std::unique_ptr<OpusEncoder, decltype(&opus_encoder_destroy)> opus(
                nullptr, opus_encoder_destroy);
            if (config.audio) {
                if (!config.synthetic)
                    loopback = std::make_unique<Loopback>();
                if (config.live)
                    aac = std::make_unique<Encoder>(sink);
                else {
                    int error = 0;
                    opus.reset(opus_encoder_create(48000, 2, OPUS_APPLICATION_AUDIO, &error));
                    if (!opus || error != OPUS_OK)
                        throw std::runtime_error("OPUS_INITIALIZATION_FAILED");
                    opus_encoder_ctl(opus.get(), OPUS_SET_BITRATE(128000));
                }
            }
            const auto origin = clock_us();
            auto next_video = origin, next_audio = origin, last_frame = origin;
            ComPtr<ID3D11Texture2D> previous_frame;
            unsigned previous_width = 0, previous_height = 0;
            std::vector<int16_t> pcm;
            int64_t audio_frames = 0, audio_origin = -1, last_audio = origin;
            while (!stopped) {
                auto now = clock_us();
                if (now >= next_video) {
                    next_video = now + 1000000 / config.fps;
                    ComPtr<ID3D11Texture2D> texture;
                    unsigned width = config.width, height = config.height;
                    if (config.synthetic) {
                        float color[]{static_cast<float>((now - origin) / 1000000 % 3 == 0),
                                      static_cast<float>((now - origin) / 1000000 % 3 == 1),
                                      static_cast<float>((now - origin) / 1000000 % 3 == 2), 1.f};
                        context->ClearRenderTargetView(canvas.Get(), color);
                        texture = synthetic;
                    } else if (auto frame = pool.TryGetNextFrame()) {
                        auto size = frame.ContentSize();
                        width = static_cast<unsigned>(size.Width);
                        height = static_cast<unsigned>(size.Height);
                        if (!width || !height)
                            throw std::runtime_error("CAPTURE_SOURCE_CLOSED");
                        auto access = frame.Surface()
                                          .as<::Windows::Graphics::DirectX::Direct3D11::
                                                  IDirect3DDxgiInterfaceAccess>();
                        check(access->GetInterface(IID_PPV_ARGS(&texture)), "WGC_TEXTURE_FAILED");
                        D3D11_TEXTURE2D_DESC desc{};
                        texture->GetDesc(&desc);
                        if (!previous_frame || width != previous_width ||
                            height != previous_height) {
                            if (previous_frame)
                                throw std::runtime_error("CAPTURE_SIZE_CHANGED_RESTART_REQUIRED");
                            desc.BindFlags = D3D11_BIND_RENDER_TARGET;
                            desc.MiscFlags = 0;
                            check(device->CreateTexture2D(&desc, nullptr, &previous_frame),
                                  "FRAME_CACHE_FAILED");
                            previous_width = width;
                            previous_height = height;
                        }
                        context->CopyResource(previous_frame.Get(), texture.Get());
                        // Consume the borrowed frame before closing it. The NV12 output has
                        // independent ownership.
                        auto converted = converter.convert(texture.Get(), width, height);
                        if (video.video(converted.Get(), now - origin, keyframe.load(),
                                        bitrate.load()))
                            keyframe = false;
                        last_frame = now;
                        texture.Reset();
                    } else if (previous_frame) {
                        texture = previous_frame;
                        width = previous_width;
                        height = previous_height;
                    }
                    if (texture) {
                        auto converted = converter.convert(texture.Get(), width, height);
                        if (video.video(converted.Get(), now - origin, keyframe.load(),
                                        bitrate.load()))
                            keyframe = false;
                        last_frame = now;
                    }
                    if (!config.synthetic && config.window && !IsWindow(config.window))
                        throw std::runtime_error("CAPTURE_SOURCE_CLOSED");
                    if (!config.synthetic && !previous_frame && now - last_frame > 5000000)
                        throw std::runtime_error("CAPTURE_SOURCE_STALLED");
                }
                video.drain();
                if (config.audio) {
                    if (config.synthetic && now >= next_audio) {
                        next_audio += 10000;
                        for (int i = 0; i < 480; ++i) {
                            auto position = audio_frames + static_cast<int64_t>(pcm.size() / 2);
                            auto sample = static_cast<int16_t>(
                                std::sin(position * 6.283185307179586 * 440 / 48000) * 4000);
                            pcm.push_back(sample);
                            pcm.push_back(sample);
                        }
                    } else if (loopback) {
                        auto samples = loopback->poll();
                        if (!samples.empty()) {
                            if (audio_origin < 0)
                                audio_origin = std::max<int64_t>(
                                    0,
                                    now - origin -
                                        static_cast<int64_t>(samples.size() / 2) * 1000000 / 48000);
                            last_audio = now;
                            pcm.insert(pcm.end(), samples.begin(), samples.end());
                        } else if (now - last_audio > 30000) {
                            // WASAPI may stop producing buffers during silence. Keep the audio
                            // clock aligned to video instead of accumulating a delay when playback
                            // resumes.
                            if (audio_origin < 0)
                                audio_origin = now - origin;
                            auto missing = (now - origin - audio_origin) * 48000 / 1000000 -
                                           audio_frames - static_cast<int64_t>(pcm.size() / 2);
                            if (missing > 0)
                                pcm.resize(
                                    pcm.size() +
                                        static_cast<size_t>(std::min<int64_t>(missing, 1920)) * 2,
                                    0);
                        }
                    }
                    if (pcm.size() > 19200)
                        throw std::runtime_error("AUDIO_BACKPRESSURE");
                    const size_t count = config.live ? 2048 : 1920;
                    while (pcm.size() >= count) {
                        const auto timestamp =
                            std::max<int64_t>(0, audio_origin) + audio_frames * 1000000 / 48000;
                        if (aac)
                            aac->audio(std::span(pcm.data(), count), timestamp);
                        else {
                            uint8_t encoded[4000];
                            int size =
                                opus_encode(opus.get(), pcm.data(), 960, encoded, sizeof(encoded));
                            if (size < 0)
                                throw std::runtime_error("OPUS_ENCODE_FAILED");
                            sink({Codec::Opus, std::span(encoded, static_cast<size_t>(size)),
                                  timestamp, false});
                        }
                        audio_frames += count / 2;
                        pcm.erase(pcm.begin(), pcm.begin() + static_cast<ptrdiff_t>(count));
                    }
                }
                std::this_thread::sleep_for(std::chrono::milliseconds(3));
            }
        } catch (const winrt::hresult_error &e) {
            if (!stopped)
                failure("CAPTURE_FAILED (HRESULT " +
                        std::to_string(static_cast<uint32_t>(e.code().value)) + ")");
        } catch (const std::exception &e) {
            if (!stopped)
                failure(e.what());
        }
    }
};
Capture::Capture(CaptureConfig c, PacketSink sink, Failure failure)
    : state_(std::make_unique<State>(c, std::move(sink), std::move(failure))) {
    if (!c.width || !c.height || c.width > 1280 || c.height > 720 || c.width % 2 || c.height % 2 ||
        !c.fps || c.fps > 30)
        throw std::runtime_error("INVALID_CAPTURE_PROFILE");
    state_->worker = std::thread([s = state_.get()] { s->run(); });
}
Capture::~Capture() {
    stop();
}
void Capture::request_keyframe() {
    state_->keyframe = true;
}
void Capture::set_bitrate(uint32_t bitrate) {
    state_->bitrate = std::clamp(bitrate, 500000u, state_->config.bitrate);
}
void Capture::stop() {
    if (state_) {
        state_->stopped = true;
        if (state_->worker.joinable())
            state_->worker.join();
    }
}
} // namespace lancast
