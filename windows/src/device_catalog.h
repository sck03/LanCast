#pragma once
#include <nlohmann/json.hpp>
#include <string>
#include <vector>

struct DiscoveredDevice {
    std::string key, name, ip, address, fingerprint, id;
    bool dlna = false;
};
// Discovery is untrusted. Normalize bounded records before presenting them to UI.
class DeviceCatalog {
  public:
    void clear() {
        entries_.clear();
    }
    void update(const nlohmann::json &devices, bool dlna);
    const std::vector<DiscoveredDevice> &entries() const {
        return entries_;
    }

  private:
    std::vector<DiscoveredDevice> entries_;
};
bool valid_fingerprint(const std::string &value);
