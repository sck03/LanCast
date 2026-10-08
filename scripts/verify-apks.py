#!/usr/bin/env python3
"""Recheck and package selected APKs; accepts the same version inputs as the builder."""
import argparse
from android_artifacts import verify_and_package
from android_products import SELECTION_LABELS, selected_products
from build_config import ROOT, add_arguments, from_args, record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_arguments(parser)
    parser.add_argument("--product", choices=SELECTION_LABELS)
    args = parser.parse_args()
    config = from_args("android", args)
    verify_and_package(ROOT, config, selected_products(args.product), record(config))


if __name__ == "__main__":
    main()
