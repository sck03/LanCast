# JNI resolves the mux class and invokes onTs on the concrete Sink implementation.
-keep class dev.lancast.sender.TsMux { *; }
-keep interface dev.lancast.sender.TsMux$Sink { *; }
-keepclassmembers class * implements dev.lancast.sender.TsMux$Sink {
    public int onTs(byte[]);
}
