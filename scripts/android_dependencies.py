"""Validate product isolation against Gradle's resolved dependency reports."""
import re


def verify_dependencies(product, report, media3_version):
    projects = set(re.findall(r"(?m)^[ |]*(?:\+---|\\---) project ['\"]?([^'\"\s]+)", report))
    if (":airplay-receiver" in projects) != product.airplay:
        raise ValueError(f"{product.key}: AirPlay dependency isolation failed")
    if product.flavor == "legacy" or product.role == "sender":
        if "androidx.media3:" in report:
            raise ValueError(f"Media3 leaked into {product.key}")
    else:
        resolved = {selected or requested for requested, selected in re.findall(
            r"\bandroidx\.media3:media3-common:([^\s]+)(?: -> ([^\s]+))?", report)}
        if resolved != {media3_version}:
            raise ValueError(f"{product.key}: Media3 version mismatch: {sorted(resolved)}")
