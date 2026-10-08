"""Android product identities shared by build, verification and Actions selection."""
import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path


@dataclass(frozen=True)
class Product:
    key: str
    name: str
    role: str
    module: str
    flavor: str
    application_id: str
    minimum_sdk: int
    features: tuple[str, ...] = ()

    @property
    def airplay(self):
        return self.flavor == "airplay"

    @property
    def artifact_prefix(self):
        return "LanCast-Android-" + self.name.replace(" ", "-")

    def variant(self, mode):
        return self.flavor.capitalize() + mode

    def apk_directory(self, root, mode):
        directory = Path(root) / "android" / self.module / "build/outputs/apk"
        return directory / self.flavor / mode.lower()


PRODUCTS = (
    Product("sender", "Sender", "sender", "app-sender", "", "dev.lancast.sender", 29, ("sender",)),
    Product("receiver-standard", "Receiver Standard", "receiver", "app-receiver", "standard", "dev.lancast.receiver", 23),
    Product("receiver-legacy", "Receiver Legacy", "receiver", "app-receiver", "legacy", "dev.lancast.receiver.legacy", 21, ("legacy",)),
    Product("receiver-airplay", "Receiver AirPlay", "receiver", "app-receiver", "airplay", "dev.lancast.receiver.airplay", 23),
)
SELECTION_LABELS = {
    "all": "全部（1 个发送端 + 3 个接收端）",
    "sender": "发送端（Android 10+）",
    "receivers": "全部接收端（Standard / Legacy / AirPlay）",
    "receiver-standard": "接收端 Standard（Android 6+）",
    "receiver-legacy": "接收端 Legacy（Android 5+）",
    "receiver-airplay": "接收端 AirPlay（Android 6+，含 GPL 模块）",
}


def selection_key(value=None, *, environ=None):
    env = os.environ if environ is None else environ
    value = value or env.get("LC_ANDROID_PRODUCT") or "all"
    value = {label: key for key, label in SELECTION_LABELS.items()}.get(value, value)
    if value not in SELECTION_LABELS:
        raise ValueError("Unknown Android product: select all, sender, receivers, or receiver-standard/legacy/airplay")
    return value


def selected_products(value=None, *, environ=None):
    key = selection_key(value, environ=environ)
    return tuple(product for product in PRODUCTS if key == "all" or product.key == key
                 or (key == "receivers" and product.role == "receiver"))


def gradle_tasks(products, mode):
    tasks = [f":{p.module}:{task}{p.variant(mode)}" for p in products for task in ("assemble", "lint")]
    tasks.append(f":control-bridge:test{mode}UnitTest")
    if any(p.role == "receiver" for p in products):
        tasks.append(":receiver-contracts:test")
    return tasks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--product", choices=SELECTION_LABELS)
    parser.add_argument("--github", action="store_true")
    args = parser.parse_args()
    key = selection_key(args.product)
    products = selected_products(key)
    if args.github:
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as stream:
            stream.write(f"LC_ANDROID_PRODUCT={key}\n")
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            stream.write(f"selection={key}\n")
            for product in PRODUCTS:
                stream.write(f"{product.key.replace('-', '_')}={str(product in products).lower()}\n")
    print(json.dumps({"selection": key, "products": [p.key for p in products]}, indent=2))


if __name__ == "__main__":
    main()
