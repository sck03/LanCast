plugins { id("com.android.application"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.lancast.sender"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    defaultConfig {
        applicationId = "dev.lancast.sender"
        minSdk = 29; targetSdk = 36
        versionCode = rootProject.extra["productBuild"] as Int
        versionName = rootProject.extra["productVersion"] as String
        ndk { abiFilters += (rootProject.extra["productAbis"] as List<*>).map { it.toString() } }
    }
    buildFeatures { buildConfig = true }
    
    buildTypes { release { isMinifyEnabled = false } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
    packaging { jniLibs { useLegacyPackaging = false } }
}
kotlin { jvmToolchain(17) }
dependencies {
    implementation(project(":control-bridge"))
    implementation(project(":media-webrtc"))
    
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
}
