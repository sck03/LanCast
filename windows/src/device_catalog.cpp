#include "device_catalog.h"
#include "network.h"
#include <algorithm>

bool valid_fingerprint(const std::string &value) {
    return value.size() == 64 && std::all_of(value.begin(), value.end(), [](unsigned char c) {
               return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F');
           });
}
void DeviceCatalog::update(const nlohmann::json &devices, bool dlna) {
    if (!devices.is_array())
        return;
    std::erase_if(entries_, [dlna](const auto &d) { return d.dlna == dlna; });
    for (const auto &d : devices) {
        if (entries_.size() >= 128)
            break;
        if (!d.is_object())
            continue;
        if (!d.contains("id") || !d["id"].is_string())
            continue;
        DiscoveredDevice item;
        item.id = d["id"].get<std::string>();
        if (item.id.empty() || item.id.size() > 512)
            continue;
        item.dlna = dlna;
        item.key = (dlna ? "dlna:" : "lancast:") + item.id;
        item.name =
            d.contains("name") && d["name"].is_string() ? d["name"].get<std::string>() : "TV";
        if (item.name.size() > 256)
            item.name.resize(256);
        std::erase_if(item.name, [](unsigned char c) { return c < 32 || c == 127; });
        const std::string suffix = "._lancast._tcp.local.";
        if (item.name.ends_with(suffix))
            item.name.resize(item.name.size() - suffix.size());
        if (dlna) {
            if (!d.contains("ip") || !d["ip"].is_string())
                continue;
            item.ip = d["ip"].get<std::string>();
        } else {
            if (!d.contains("addresses") || !d["addresses"].is_array() || !d.contains("port") ||
                !d["port"].is_number_integer())
                continue;
            const auto port = d["port"].get<int64_t>();
            if (port < 1 || port > 65535)
                continue;
            for (const auto &address : d["addresses"]) {
                if (address.is_string() && network::lan_ipv4(address.get<std::string>())) {
                    item.ip = address.get<std::string>();
                    break;
                }
            }
            item.address = item.ip + ":" + std::to_string(port);
            if (d.contains("fingerprint") && d["fingerprint"].is_string()) {
                const auto pin = d["fingerprint"].get<std::string>();
                if (valid_fingerprint(pin))
                    item.fingerprint = pin;
            }
            if (item.fingerprint.empty())
                continue;
        }
        if (!network::lan_ipv4(item.ip))
            continue;
        if (std::none_of(entries_.begin(), entries_.end(),
                         [&](const auto &v) { return v.key == item.key; }))
            entries_.push_back(std::move(item));
    }
    std::sort(entries_.begin(), entries_.end(), [](const auto &a, const auto &b) {
        return a.dlna != b.dlna ? !a.dlna : a.key < b.key;
    });
}
