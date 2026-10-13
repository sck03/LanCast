plugins { alias(libs.plugins.android.library) }
android {
    namespace = "dev.lancast.player.legacy"
    compileSdk = libs.versions.compileSdk.get().toInt()
    buildToolsVersion = libs.versions.buildTools.get()
    defaultConfig { minSdk = 21 }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
}
dependencies { coreLibraryDesugaring(libs.desugar)
    implementation(project(":player-api"))
    implementation(project(":control-bridge"))
}
