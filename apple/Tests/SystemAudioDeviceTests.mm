#import <XCTest/XCTest.h>
#import "SystemAudioDevice.h"
#include "AudioTimeline.h"
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
- (void)testSourceTimelinePreservesGapsAndRejectsBackwardsOrInvalidTimes {
    LCAudioTimeline timeline;
    XCTAssertTrue(timeline.begin(100, .01)); XCTAssertEqual(timeline.take(480), 0);
    XCTAssertTrue(timeline.begin(100.01, .01)); XCTAssertEqual(timeline.take(480), 480);
    XCTAssertTrue(timeline.begin(100.10, .01)); XCTAssertTrue(timeline.hasGap());
    XCTAssertEqual(timeline.take(480), 4800);
    XCTAssertFalse(timeline.begin(100.09, .01)); XCTAssertFalse(timeline.begin(100.10, .01));
    XCTAssertFalse(timeline.begin(NAN, .01)); XCTAssertFalse(timeline.begin(-1, .01));
    XCTAssertFalse(timeline.begin(100.11, NAN)); XCTAssertFalse(timeline.begin(100.11, 0));
    XCTAssertTrue(timeline.begin(100.11, .01)); XCTAssertEqual(timeline.take(480), 5280);
    timeline.reset(); XCTAssertTrue(timeline.begin(1, .01)); XCTAssertEqual(timeline.take(480), 0);
}
- (void)testContinuousSourceDoesNotResetResamplerPriming {
    LCAudioTimeline timeline;
    XCTAssertTrue(timeline.begin(100, 441.0 / 44100));
    XCTAssertEqual(timeline.take(400), 0); // First conversion retains its priming samples.
    XCTAssertTrue(timeline.begin(100.01, 441.0 / 44100));
    XCTAssertFalse(timeline.hasGap()); XCTAssertEqual(timeline.take(480), 400);
    XCTAssertTrue(timeline.begin(100.02, .01));
    XCTAssertFalse(timeline.hasGap()); XCTAssertEqual(timeline.take(480), 880);
    XCTAssertTrue(timeline.begin(100.10, .01));
    XCTAssertTrue(timeline.hasGap()); XCTAssertEqual(timeline.take(480), 4800);
}
- (void)testResamplesMonoFloatToStereoPCMAndTerminates {
    XCTestExpectation *received = [self expectationWithDescription:@"converted PCM reached ADM"];
    auto calls = std::make_shared<std::atomic<int>>(0);
    AudioProbe *probe = [AudioProbe new];
    probe.deliverRecordedData = ^OSStatus(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *stamp,
            NSInteger bus, UInt32 frames, const AudioBufferList *data, void *context,
            LKRTCAudioDeviceRenderRecordedDataBlock render) {
        XCTAssertGreaterThan(frames, 0u); XCTAssertLessThanOrEqual(frames, 480u);
        if (!data || !frames || frames > 480) { XCTFail(@"Invalid converted audio"); return -1; }
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
        kCFAllocatorDefault, nullptr, 0, samples * sizeof(float), kCMBlockBufferAssureMemoryNowFlag, &block), noErr);
    if (!block) { [device terminateDevice]; return; }
    char *bytes = nullptr;
    XCTAssertEqual(CMBlockBufferGetDataPointer(block, 0, nullptr, nullptr, &bytes), noErr);
    if (!bytes) { [device terminateDevice]; CFRelease(block); return; }
    for (size_t i = 0; i < samples; ++i) reinterpret_cast<float *>(bytes)[i] = .25f;
    AudioStreamBasicDescription asbd = {44100, kAudioFormatLinearPCM,
        kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked, 4, 1, 4, 1, 32, 0};
    CMAudioFormatDescriptionRef format = nullptr;
    XCTAssertEqual(CMAudioFormatDescriptionCreate(kCFAllocatorDefault, &asbd, 0, nullptr, 0, nullptr, nullptr, &format), noErr);
    if (!format) { [device terminateDevice]; CFRelease(block); return; }
    CMSampleTimingInfo timing = {CMTimeMake(1, 44100), kCMTimeZero, kCMTimeInvalid};
    size_t size = sizeof(float); CMSampleBufferRef sample = nullptr;
    XCTAssertEqual(CMSampleBufferCreateReady(kCFAllocatorDefault, block, format, samples, 1, &timing, 1, &size, &sample), noErr);
    if (!sample) { [device terminateDevice]; CFRelease(format); CFRelease(block); return; }
    [device pushSample:sample];
    [self waitForExpectations:@[received] timeout:5];
    XCTAssertTrue([device terminateDevice]);
    int stoppedCount = calls->load();
    [device pushSample:sample];
    XCTAssertEqual(calls->load(), stoppedCount);
    XCTAssertFalse(device.isInitialized); XCTAssertFalse(device.isRecording);
    CFRelease(sample); CFRelease(format); CFRelease(block);
}
- (void)testStopRecordingDiscardsTheRestOfAnInFlightConversion {
    XCTestExpectation *first = [self expectationWithDescription:@"first block stopped recording"];
    auto calls = std::make_shared<std::atomic<int>>(0);
    LCSystemAudioDevice *device = [LCSystemAudioDevice new];
    AudioProbe *probe = [AudioProbe new];
    probe.deliverRecordedData = ^OSStatus(AudioUnitRenderActionFlags *flags, const AudioTimeStamp *stamp,
            NSInteger bus, UInt32 frames, const AudioBufferList *data, void *context,
            LKRTCAudioDeviceRenderRecordedDataBlock render) {
        if (calls->fetch_add(1) == 0) { XCTAssertTrue([device stopRecording]); [first fulfill]; }
        return noErr;
    };
    XCTAssertTrue([device initializeWithDelegate:probe]);
    XCTAssertTrue([device startRecording]);
    const size_t samples = 9600;
    CMBlockBufferRef block = nullptr;
    XCTAssertEqual(CMBlockBufferCreateWithMemoryBlock(kCFAllocatorDefault, nullptr, samples * 4,
        kCFAllocatorDefault, nullptr, 0, samples * 4, kCMBlockBufferAssureMemoryNowFlag, &block), noErr);
    if (!block) { [device terminateDevice]; return; }
    AudioStreamBasicDescription asbd = {48000, kAudioFormatLinearPCM,
        kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked, 4, 1, 4, 2, 16, 0};
    CMAudioFormatDescriptionRef format = nullptr;
    XCTAssertEqual(CMAudioFormatDescriptionCreate(kCFAllocatorDefault, &asbd, 0, nullptr, 0, nullptr, nullptr, &format), noErr);
    if (!format) { [device terminateDevice]; CFRelease(block); return; }
    CMSampleTimingInfo timing = {CMTimeMake(1, 48000), kCMTimeZero, kCMTimeInvalid};
    size_t size = 4; CMSampleBufferRef sample = nullptr;
    XCTAssertEqual(CMSampleBufferCreateReady(kCFAllocatorDefault, block, format, samples, 1, &timing, 1, &size, &sample), noErr);
    if (sample) {
        [device pushSample:sample];
        [self waitForExpectations:@[first] timeout:5];
        XCTAssertTrue([device terminateDevice]);
        XCTAssertEqual(calls->load(), 1);
        CFRelease(sample);
    } else [device terminateDevice];
    // Break the delegate/test ownership cycle after its worker has joined.
    probe.deliverRecordedData = nil;
    CFRelease(format); CFRelease(block);
}
@end
