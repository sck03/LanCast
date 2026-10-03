#import <CoreMedia/CoreMedia.h>
#import <LiveKitWebRTC/LiveKitWebRTC.h>

/// Sender-only input. No microphone or audio output is ever opened by this device.
@interface LCSystemAudioDevice : NSObject <LKRTCAudioDevice>
- (void)pushSample:(CMSampleBufferRef)sample;
@end
