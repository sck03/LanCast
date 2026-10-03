#import <XCTest/XCTest.h>
#import "SystemAudioDevice.h"
#include <atomic>
#include <cmath>
#include <memory>

@interface AudioProbe : NSObject <LKRTCAudioDeviceDelegate>
@property(copy) LKRTCAudioDeviceDeliverRecordedDataBlock deliverRecordedData;
@property(copy) LKRTCAudioDeviceGetPlayoutDataBlock getPlayoutData;
@end
@implementation AudioProbe
- (double)preferredInputSampleRate { return 48000; }
- (double)preferredOutputSampleRate { return 48000; }
- (NSTimeInterval)preferredInputIOBufferDuration { return .01; }
- (NSTimeInterval)preferredOutputIOBufferDuration { return .01; }
- (void)notifyAudioInputParametersChange {}
- (void)notifyAudioOutputParametersChange {}
- (void)notifyAudioInputInterrupted {}
- (void)notifyAudioOutputInterrupted {}
- (void)dispatchAsync:(dispatch_block_t)block { block(); }
- (void)dispatchSync:(dispatch_block_t)block { block(); }
@end

@interface SystemAudioDeviceTests : XCTestCase
@end
@implementation SystemAudioDeviceTests
- (void)testResamplesMonoFloatToStereoPCMAndTerminates {
    XCTestExpectation *received = [self expectationWithDescription:@"converted PCM reached ADM"];
    auto calls = std::make_shared<std::atomic<int>>(0);
    AudioProbe *probe = [AudioProbe new];
    probe.deliverRecordedData = ^OSStatus(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *stamp,
            NSInteger bus, UInt32 frames, const AudioBufferList *data, void *context,
            LKRTCAudioDeviceRenderRecordedDataBlock render) {
        XCTAssertGreaterThan(frames, 0u); XCTAssertLessThanOrEqual(frames, 480u);
        XCTAssertEqual(data->mNumberBuffers, 1u);
        XCTAssertEqual(data->mBuffers[0].mNumberChannels, 2u);
        const int16_t *pcm = static_cast<const int16_t *>(data->mBuffers[0].mData);
        XCTAssertLessThan(std::abs(int(pcm[frames]) - 8192), 200);
        XCTAssertEqual(pcm[frames], pcm[frames + 1]);
        if (calls->fetch_add(1) == 4) [received fulfill];
        return noErr;
    };
    LCSystemAudioDevice *device = [LCSystemAudioDevice new];
    XCTAssertTrue([device initializeWithDelegate:probe]);
    XCTAssertTrue([device startRecording]);
    const size_t samples = 8820;
    CMBlockBufferRef block = nullptr;
    XCTAssertEqual(CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, nullptr, samples * sizeof(float),
        kCFAllocatorDefault, nullptr, 0, samples * sizeof(float), 0, &block), noErr);
    char *bytes = nullptr;
    XCTAssertEqual(CMBlockBufferGetDataPointer(block, 0, nullptr, nullptr, &bytes), noErr);
    for (size_t i = 0; i < samples; ++i) reinterpret_cast<float *>(bytes)[i] = .25f;
    AudioStreamBasicDescription asbd = {44100, kAudioFormatLinearPCM,
        kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked, 4, 1, 4, 1, 32, 0};
    CMAudioFormatDescriptionRef format = nullptr;
    XCTAssertEqual(CMAudioFormatDescriptionCreate(kCFAllocatorDefault, &asbd, 0, nullptr, 0, nullptr, nullptr, &format), noErr);
    CMSampleTimingInfo timing = {CMTimeMake(1, 44100), kCMTimeZero, kCMTimeInvalid};
    size_t size = sizeof(float); CMSampleBufferRef sample = nullptr;
    XCTAssertEqual(CMSampleBufferCreateReady(kCFAllocatorDefault, block, format, samples, 1, &timing, 1, &size, &sample), noErr);
    [device pushSample:sample];
    [self waitForExpectations:@[received] timeout:5];
    XCTAssertTrue([device terminateDevice]);
    int stoppedCount = calls->load();
    [device pushSample:sample];
    XCTAssertEqual(calls->load(), stoppedCount);
    XCTAssertFalse(device.isInitialized); XCTAssertFalse(device.isRecording);
    CFRelease(sample); CFRelease(format); CFRelease(block);
}
@end
