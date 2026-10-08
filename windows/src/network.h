#pragma once
#include <string>
#include <vector>
namespace network {
struct Interface {
    std::string ip;
    std::wstring name;
    unsigned long index;
    unsigned long metric;
    bool preferred;
};
bool lan_ipv4(const std::string &ip);
std::vector<Interface> interfaces();
// Resolve the route locally; no packet is sent to the destination.
std::string source_for(const std::string &destination, const std::vector<Interface> &available);
} // namespace network
