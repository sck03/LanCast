#include "device_catalog.h"
#include "network.h"
#include <iostream>
#include <stdexcept>
using Json = nlohmann::json;
static void check(bool value, const char *message) {
    if (!value)
        throw std::runtime_error(message);
}
int main() {
    try {
        check(network::lan_ipv4("192.168.1.2") && network::lan_ipv4("10.1.2.3") &&
                  network::lan_ipv4("172.16.0.1"),
              "private IPv4");
        check(!network::lan_ipv4("127.0.0.1") && !network::lan_ipv4("0.0.0.0") &&
                  !network::lan_ipv4("8.8.8.8") && !network::lan_ipv4("garbage"),
              "reject unusable interfaces");
        DeviceCatalog catalog;
        Json native = {{"id", "tv"},
                       {"name", "Living room._lancast._tcp.local."},
                       {"addresses", {"::1", "192.168.1.8"}},
                       {"port", 8787},
                       {"fingerprint", std::string(64, 'a')}};
        Json dlna = {{"id", "tv"},
                     {"name", "Living room"},
                     {"ip", "192.168.1.8"},
                     {"transport", "http://192.168.1.8/control"}};
        catalog.update(Json::array({native, native}), false);
        catalog.update(Json::array({dlna}), true);
        check(catalog.entries().size() == 2,
              "merge protocols without duplicate entries or erasing earlier results");
        check(catalog.entries()[0].name == "Living room" &&
                  catalog.entries()[0].address == "192.168.1.8:8787",
              "usable endpoint instead of first IPv6 address");
        check(valid_fingerprint(catalog.entries()[0].fingerprint), "pin hint retained");
        native["fingerprint"] = nullptr;
        catalog.update(Json::array({native}), false);
        check(catalog.entries().size() == 1 && catalog.entries()[0].dlna,
              "receivers without a full identity are not selectable");
        native["addresses"] = Json::array();
        catalog.update(Json::array({native, {{"id", 123}}, {{"id", "bad"}, {"port", "oops"}}}),
                       false);
        check(catalog.entries().size() == 1 && catalog.entries()[0].dlna,
              "malformed discovery cannot crash or select unsafe addresses");
        check(!valid_fingerprint(std::string(64, 'g')) && !valid_fingerprint("abcd"),
              "invalid pins rejected");
        check(network::source_for("not-an-address", {}).empty(),
              "route lookup rejects malformed address");
        std::cout << "desktop discovery/network contracts passed\n";
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
