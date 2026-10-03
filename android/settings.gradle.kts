pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS); repositories { google(); mavenCentral() } }
rootProject.name = "LanCast"
include(":app-receiver", ":app-sender", ":control-bridge", ":player-api", ":player-standard", ":player-legacy", ":media-webrtc")
