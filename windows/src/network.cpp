#include "network.h"
#include <winsock2.h>

#include <ws2tcpip.h>

#include <algorithm>
#include <iphlpapi.h>
#include <stdexcept>

namespace network {
bool lan_ipv4(const std::string &ip) {
    IN_ADDR address{};
    if (InetPtonA(AF_INET, ip.c_str(), &address) != 1)
        return false;
    const auto n = ntohl(address.S_un.S_addr);
    return (n >> 24) == 10 || (n >> 20) == 0xac1 || (n >> 16) == 0xc0a8 || (n >> 16) == 0xa9fe;
}
std::vector<Interface> interfaces() {
    ULONG size = 16384;
    std::vector<unsigned char> buffer(size);
    ULONG result = ERROR_BUFFER_OVERFLOW;
    for (int attempt = 0; attempt < 3 && result == ERROR_BUFFER_OVERFLOW; ++attempt) {
        buffer.resize(size);
        result =
            GetAdaptersAddresses(AF_INET, GAA_FLAG_INCLUDE_GATEWAYS, nullptr,
                                 reinterpret_cast<IP_ADAPTER_ADDRESSES *>(buffer.data()), &size);
    }
    if (result == ERROR_NO_DATA)
        return {};
    if (result != NO_ERROR)
        throw std::runtime_error("无法读取网络连接，请检查 Wi-Fi 或网线后重试");
    std::vector<Interface> found;
    for (auto *adapter = reinterpret_cast<IP_ADAPTER_ADDRESSES *>(buffer.data()); adapter;
         adapter = adapter->Next) {
        if (adapter->OperStatus != IfOperStatusUp || adapter->IfType == IF_TYPE_SOFTWARE_LOOPBACK ||
            adapter->IfType == IF_TYPE_TUNNEL)
            continue;
        for (auto *entry = adapter->FirstUnicastAddress; entry; entry = entry->Next) {
            if (entry->Address.lpSockaddr->sa_family != AF_INET ||
                entry->DadState != IpDadStatePreferred)
                continue;
            char ip[INET_ADDRSTRLEN]{};
            auto *address = reinterpret_cast<sockaddr_in *>(entry->Address.lpSockaddr);
            if (!InetNtopA(AF_INET, &address->sin_addr, ip, sizeof(ip)) || !lan_ipv4(ip))
                continue;
            found.push_back({ip, adapter->FriendlyName ? adapter->FriendlyName : L"网络连接",
                             adapter->IfIndex, adapter->Ipv4Metric,
                             adapter->FirstGatewayAddress != nullptr &&
                                 (adapter->IfType == IF_TYPE_ETHERNET_CSMACD ||
                                  adapter->IfType == IF_TYPE_IEEE80211)});
        }
    }
    std::stable_sort(found.begin(), found.end(), [](const auto &a, const auto &b) {
        if (a.preferred != b.preferred)
            return a.preferred;
        if (a.metric != b.metric)
            return a.metric < b.metric;
        return a.ip < b.ip;
    });
    return found;
}
std::string source_for(const std::string &destination, const std::vector<Interface> &available) {
    SOCKADDR_INET remote{}, source{};
    remote.Ipv4.sin_family = AF_INET;
    if (InetPtonA(AF_INET, destination.c_str(), &remote.Ipv4.sin_addr) != 1)
        return {};
    MIB_IPFORWARD_ROW2 route{};
    if (GetBestRoute2(nullptr, 0, nullptr, &remote, 0, &route, &source) != NO_ERROR)
        return {};
    char ip[INET_ADDRSTRLEN]{};
    if (!InetNtopA(AF_INET, &source.Ipv4.sin_addr, ip, sizeof(ip)))
        return {};
    const auto entry = std::find_if(available.begin(), available.end(),
                                    [&](const auto &item) { return item.ip == ip; });
    return entry == available.end() ? std::string{} : entry->ip;
}
} // namespace network
