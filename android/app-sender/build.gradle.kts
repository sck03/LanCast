plugins { alias(libs.plugins.android.application) }
android {
    namespace = "dev.lancast.sender"
    compileSdk = libs.versions.compileSdk.get().toInt()
    ndkVersion = libs.versions.ndk.get()
    buildToolsVersion = libs.versions.buildTools.get()
    defaultConfig {
        applicationId = "dev.lancast.sender"
        minSdk = 29; targetSdk = 36
        versionCode = rootProject.extra["productBuild"] as Int
        versionName = rootProject.extra["productVersion"] as String
        ndk { abiFilters += (rootProject.extra["productAbis"] as List<*>).map { it.toString() } }
    }
    buildFeatures { buildConfig = true }
    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
    packaging { jniLibs { useLegacyPackaging = false } }
}
dependencies {
    implementation(project(":control-bridge"))
    implementation(project(":media-webrtc"))
    testImplementation("junit:junit:4.13.2")
    coreLibraryDesugaring(libs.desugar)
}
