# The pinned prebuilt AAR includes native-to-Java lookups throughout org.webrtc.
# Keep that ABI until a custom-source RTC build can supply a narrower verified set.
-keep class org.webrtc.** { *; }
