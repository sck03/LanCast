"""Explicit Apple roles and Xcode delivery directories; never infer products from stale files."""
from dataclasses import dataclass


@dataclass(frozen=True)
class AppleProduct:
    scheme: str
    role: str
    bundle_id: str
    minimum_os: str
    sdks: tuple[tuple[str, str], ...]
    rust_features: str

    def directories(self, configuration):
        return tuple((configuration + suffix, kind) for suffix, kind in self.sdks)

    def archive_name(self, kind, label):
        role = "Receiver" if self.role == "receiver" else "Sender-Receiver"
        return f"{self.scheme}-{role}-{kind}-{label}.zip"


PRODUCTS = {
    "macos": AppleProduct("LanCastMac", "sender-receiver", "dev.lancast.mac", "13.0", (("", "Universal"),), "sender,legacy"),
    "ios": AppleProduct("LanCastIOS", "sender-receiver", "dev.lancast.ios", "16.0", (("-iphoneos", "Device"), ("-iphonesimulator", "Simulator")), "sender,legacy"),
    "tvos": AppleProduct("LanCastTV", "receiver", "dev.lancast.tv", "17.0", (("-appletvos", "Device"), ("-appletvsimulator", "Simulator")), "legacy"),
}


def app_paths(root, platform, configuration):
    product = PRODUCTS[platform]
    return tuple((root / directory / f"{product.scheme}.app", kind)
                 for directory, kind in product.directories(configuration))
