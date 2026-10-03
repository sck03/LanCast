#import "SystemAudioDevice.h"
#import <AVFoundation/AVFoundation.h>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <thread>

@implementation LCSystemAudioDevice {
    std::mutex _mutex;
    std::condition_variable _ready;
    std::deque<CMSampleBufferRef> _samples;
    std::thread _thread;
    id<LKRTCAudioDeviceDelegate> _delegate;
    bool _exit, _recording;
}
- (double)deviceInputSampleRate { return 48000; }
- (double)deviceOutputSampleRate { return 48000; }
- (NSInteger)inputNumberOfChannels { return 2; }
- (NSInteger)outputNumberOfChannels { return 2; }
- (NSTimeInterval)inputIOBufferDuration { return .01; }
- (NSTimeInterval)outputIOBufferDuration { return .01; }
- (NSTimeInterval)inputLatency { return .01; }
- (NSTimeInterval)outputLatency { return .01; }
- (BOOL)isInitialized { std::lock_guard guard(_mutex); return _delegate != nil; }
- (BOOL)isRecording { std::lock_guard guard(_mutex); return _recording; }
- (BOOL)isRecordingInitialized { return YES; }
- (BOOL)isPlayoutInitialized { return YES; }
- (BOOL)isPlaying { return NO; }
- (BOOL)initializePlayout { return YES; }
- (BOOL)startPlayout { return YES; }
- (BOOL)stopPlayout { return YES; }
- (BOOL)initializeRecording { return YES; }
- (BOOL)startRecording { std::lock_guard guard(_mutex); _recording = true; return YES; }
- (BOOL)stopRecording {
    std::lock_guard guard(_mutex); _recording = false;
    for (auto sample : _samples) CFRelease(sample);
    _samples.clear(); return YES;
}
- (BOOL)initializeWithDelegate:(id<LKRTCAudioDeviceDelegate>)delegate {
    std::lock_guard guard(_mutex);
    if (_thread.joinable()) return NO;
    _delegate = delegate; _exit = false;
    // One OS thread owns conversion and all deliverRecordedData calls, as ADM requires.
    _thread = std::thread([self] { [self consume]; });
    return YES;
}
- (void)pushSample:(CMSampleBufferRef)sample {
    if (!CMSampleBufferIsValid(sample) || CMSampleBufferGetNumSamples(sample) > 19200) return;
    std::lock_guard guard(_mutex);
    if (!_recording || _exit || !_delegate) return;
    // At most four callbacks retained. Overflow discards oldest audio, never grows latency.
    if (_samples.size() == 4) { CFRelease(_samples.front()); _samples.pop_front(); }
    CFRetain(sample); _samples.push_back(sample); _ready.notify_one();
}
- (void)consume {
    @autoreleasepool {
        AVAudioFormat *output = [[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:48000 channels:2 interleaved:YES];
        AVAudioConverter *converter = nil;
        double timeline = 0;
        for (;;) {
            CMSampleBufferRef sample;
            id<LKRTCAudioDeviceDelegate> delegate;
            {
                std::unique_lock lock(_mutex);
                _ready.wait(lock, [&] { return _exit || !_samples.empty(); });
                if (_exit) break;
                sample = _samples.front(); _samples.pop_front(); delegate = _delegate;
            }
            @autoreleasepool {
                const AudioStreamBasicDescription *asbd = CMAudioFormatDescriptionGetStreamBasicDescription(CMSampleBufferGetFormatDescription(sample));
                AVAudioFormat *inputFormat = asbd ? [[AVAudioFormat alloc] initWithStreamDescription:asbd] : nil;
                auto count = (AVAudioFrameCount)CMSampleBufferGetNumSamples(sample);
                if (inputFormat && count && asbd->mSampleRate >= 8000 && asbd->mSampleRate <= 192000) {
                    AVAudioPCMBuffer *input = [[AVAudioPCMBuffer alloc] initWithPCMFormat:inputFormat frameCapacity:count];
                    input.frameLength = count;
                    if (CMSampleBufferCopyPCMDataIntoAudioBufferList(sample, 0, count, input.mutableAudioBufferList) == noErr) {
                        if (!converter || ![converter.inputFormat isEqual:inputFormat])
                            converter = [[AVAudioConverter alloc] initFromFormat:inputFormat toFormat:output];
                        AVAudioPCMBuffer *pcm = [[AVAudioPCMBuffer alloc] initWithPCMFormat:output frameCapacity:480];
                        __block BOOL supplied = NO;
                        // Resampler can emit several 10 ms blocks from one source callback.
                        for (int i = 0; converter && i < 50; ++i) {
                            NSError *error = nil;
                            AVAudioConverterOutputStatus result = [converter convertToBuffer:pcm error:&error withInputFromBlock:^AVAudioBuffer *(AVAudioPacketCount packets, AVAudioConverterInputStatus *status) {
                                if (supplied) { *status = AVAudioConverterInputStatus_NoDataNow; return nil; }
                                supplied = YES; *status = AVAudioConverterInputStatus_HaveData; return input;
                            }];
                            if (error || !pcm.frameLength) break;
                            AudioUnitRenderActionFlags flags = 0;
                            AudioTimeStamp stamp = {}; stamp.mSampleTime = timeline; stamp.mFlags = kAudioTimeStampSampleTimeValid;
                            timeline += pcm.frameLength;
                            delegate.deliverRecordedData(&flags, &stamp, 0, pcm.frameLength, pcm.audioBufferList, nullptr, nil);
                            if (result == AVAudioConverterOutputStatus_InputRanDry || result == AVAudioConverterOutputStatus_EndOfStream) break;
                        }
                    }
                }
                CFRelease(sample);
            }
        }
    }
}
- (BOOL)terminateDevice {
    {
        std::lock_guard guard(_mutex); _exit = true; _recording = false; _ready.notify_all();
    }
    if (_thread.joinable()) _thread.join();
    std::lock_guard guard(_mutex);
    for (auto sample : _samples) CFRelease(sample);
    _samples.clear(); _delegate = nil; return YES;
}
- (void)dealloc { [self terminateDevice]; }
@end
