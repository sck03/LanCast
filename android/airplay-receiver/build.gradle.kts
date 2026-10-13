plugins { alias(libs.plugins.android.library) }
android {
    namespace = "dev.lancast.airplay"
    compileSdk = libs.versions.compileSdk.get().toInt()
    defaultConfig { minSdk = 23; consumerProguardFiles("consumer-rules.pro") }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
}
dependencies {
    implementation(project(":receiver-contracts"))
    testImplementation("junit:junit:4.13.2")
}
