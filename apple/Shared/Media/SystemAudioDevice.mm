#import "SystemAudioDevice.h"
#include "AudioTimeline.h"
#import <AVFoundation/AVFoundation.h>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <thread>
#include <cstdint>

struct LCQueuedAudioSample {
    CMSampleBufferRef sample;
    uint64_t epoch;
};

@implementation LCSystemAudioDevice {
    std::mutex _mutex;
    std::recursive_mutex _deliveryMutex;
    std::condition_variable _ready;
    std::deque<LCQueuedAudioSample> _samples;
    std::thread _thread;
    id<LKRTCAudioDeviceDelegate> _delegate;
    bool _exit, _recording;
    uint64_t _recordingEpoch;
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
- (BOOL)startRecording { std::lock_guard guard(_mutex); if (_exit || !_delegate) return NO; _recording = true; return YES; }
- (BOOL)stopRecording {
    std::lock_guard delivery(_deliveryMutex);
    std::lock_guard guard(_mutex); _recording = false; ++_recordingEpoch;
    for (auto item : _samples) CFRelease(item.sample);
    _samples.clear(); return YES;
}
- (BOOL)initializeWithDelegate:(id<LKRTCAudioDeviceDelegate>)delegate {
    std::lock_guard guard(_mutex);
    if (_thread.joinable()) return NO;
    _delegate = delegate; _exit = false; _recording = false; ++_recordingEpoch;
    // One OS thread owns conversion and all deliverRecordedData calls, as ADM requires.
    _thread = std::thread([self] { [self consume]; });
    return YES;
}
- (void)pushSample:(CMSampleBufferRef)sample {
    if (!CMSampleBufferIsValid(sample) || CMSampleBufferGetNumSamples(sample) > 19200) return;
    std::lock_guard guard(_mutex);
    if (!_recording || _exit || !_delegate) return;
    // At most four callbacks retained. Overflow discards oldest audio, never grows latency.
    if (_samples.size() == 4) { CFRelease(_samples.front().sample); _samples.pop_front(); }
    CFRetain(sample); _samples.push_back({sample, _recordingEpoch}); _ready.notify_one();
}
- (void)consume {
    @autoreleasepool {
        AVAudioFormat *output = [[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatInt16 sampleRate:48000 channels:2 interleaved:YES];
        AVAudioConverter *converter = nil;
        LCAudioTimeline timeline;
        uint64_t activeEpoch = 0;
        for (;;) {
            CMSampleBufferRef sample;
            uint64_t sampleEpoch;
            id<LKRTCAudioDeviceDelegate> delegate;
            {
                std::unique_lock lock(_mutex);
                _ready.wait(lock, [&] { return _exit || !_samples.empty(); });
                if (_exit) break;
                auto item = _samples.front(); _samples.pop_front(); sample = item.sample; sampleEpoch = item.epoch; delegate = _delegate;
            }
            @autoreleasepool {
                {
                    std::lock_guard guard(_mutex);
                    if (!_recording || sampleEpoch != _recordingEpoch || !_delegate) {
                        CFRelease(sample);
                        continue;
                    }
                    if (sampleEpoch != activeEpoch) {
                        activeEpoch = sampleEpoch;
                        timeline.reset();
                        converter = nil;
                    }
                }
                const AudioStreamBasicDescription *asbd = CMAudioFormatDescriptionGetStreamBasicDescription(CMSampleBufferGetFormatDescription(sample));
                AVAudioFormat *inputFormat = asbd ? [[AVAudioFormat alloc] initWithStreamDescription:asbd] : nil;
                auto count = (AVAudioFrameCount)CMSampleBufferGetNumSamples(sample);
                if (inputFormat && count && asbd->mSampleRate >= 8000 && asbd->mSampleRate <= 192000) {
                    const CMTime pts = CMSampleBufferGetPresentationTimeStamp(sample);
                    if (!CMTIME_IS_NUMERIC(pts) || !timeline.begin(CMTimeGetSeconds(pts))) {
                        CFRelease(sample);
                        continue;
                    }
                    // A dropped source callback must not compress time or retain old resampler history.
                    if (timeline.hasGap()) converter = nil;
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
                            AudioTimeStamp stamp = {}; stamp.mSampleTime = timeline.take(pcm.frameLength); stamp.mFlags = kAudioTimeStampSampleTimeValid;
                            // Stop/start cannot let the remainder of an old conversion enter a new recording.
                            std::lock_guard delivery(_deliveryMutex);
                            bool deliver = false;
                            {
                                std::lock_guard guard(_mutex);
                                deliver = _recording && sampleEpoch == _recordingEpoch && delegate == _delegate;
                            }
                            if (deliver) delegate.deliverRecordedData(&flags, &stamp, 0, pcm.frameLength, pcm.audioBufferList, nullptr, nil);
                            else break;
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
    for (auto item : _samples) CFRelease(item.sample);
    _samples.clear(); _delegate = nil; return YES;
}
- (void)dealloc { [self terminateDevice]; }
@end
