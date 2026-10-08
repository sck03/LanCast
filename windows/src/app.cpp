#include "app.h"
#include "device_catalog.h"
#include "lancast.h"
#include "media.h"
#include "network.h"
#include "tray.h"
#include "view.h"
#include <algorithm>
#include <commctrl.h>
#include <commdlg.h>
#include <filesystem>
#include <memory>
#include <nlohmann/json.hpp>
#include <objbase.h>
#include <shlobj.h>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#include <windows.h>
using Json = nlohmann::json;
namespace {
class DesktopApp {
    static constexpr UINT MEDIA_EVENT = WM_APP + 1;
    HWND main_window{}, status_text{}, invitation{}, windows_list{}, audio_check{}, devices_list{};
    LancastHandle core = 0;
    static LancastHandle create_core() {
        LancastConfig c{sizeof(LancastConfig), 2, 0};
        return lancast_create_v2(&c);
    }
    std::unique_ptr<MediaSender> media;
    std::string session, mode, dlna_id, dlna_ip, local_address, receiver_address,
        receiver_fingerprint;
    Json shared_file;
    bool connected = false;
    uint64_t generation = 0;
    bool live_pending = false, probe = false, probe_prompted = false, selected_audio = true;
    bool profile_pending = false;
    std::string pending_session;
    std::vector<HMONITOR> monitors;
    static void release_core(LancastHandle handle) {
        lancast_shutdown(handle);
        std::thread([handle] { lancast_destroy(handle); }).detach();
    }
    std::vector<HWND> windows;
    static std::wstring wide(const std::string &text) {
        if (text.empty())
            return {};
        int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(),
                                    static_cast<int>(text.size()), nullptr, 0);
        std::wstring result(n, 0);
        MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), result.data(),
                            n);
        return result;
    }
    static std::string utf8(const std::wstring &text) {
        int n = WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), nullptr,
                                    0, nullptr, nullptr);
        std::string result(n, 0);
        WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), result.data(),
                            n, nullptr, nullptr);
        return result;
    }
    static std::string text(HWND control) {
        const int n = GetWindowTextLengthW(control);
        std::wstring value(n + 1, 0);
        GetWindowTextW(control, value.data(), n + 1);
        value.resize(n);
        return utf8(value);
    }
    void status(const std::string &value) {
        SetWindowTextW(status_text, wide(value).c_str());
        tray.update(wide(value));
        update_controls();
    }
    void command(const std::string &op, Json body = Json::object()) {
        body["op"] = op;
        auto s = body.dump();
        if (lancast_command(core, reinterpret_cast<const uint8_t *>(s.data()), s.size()) != 0)
            throw std::runtime_error("控制请求队列已满或请求无效");
    }
    static std::string uuid() {
        GUID id;
        CoCreateGuid(&id);
        wchar_t buffer[40];
        StringFromGUID2(id, buffer, 40);
        return utf8(std::wstring(buffer + 1, 36));
    }
    std::string send(const std::string &type, Json body = Json::object()) {
        const auto request = uuid();
        command("send", {{"message",
                          {{"version", 1},
                           {"id", request},
                           {"type", type},
                           {"sessionId", session.empty() ? Json(nullptr) : Json(session)},
                           {"body", body}}}});
        return request;
    }
    void stop() {
        ++generation;
        media.reset();
        live_pending = false;
        profile_pending = false;
        pending_session.clear();
        probe = false;
        probe_prompted = false;
        try {
            if (!session.empty())
                send("session.stop", {{"reason", "sender_stopped"}});
        } catch (...) {
        }
        session.clear();
        shared_file = nullptr;
        connected = false;
        pairing = false;
        file_pending = false;
        stopping = true;
        try {
            command("stop");
        } catch (...) {
        }
        status("正在停止分享…");
    }
    static void post_media(HWND target, uint64_t active, std::string type, Json body) {
        auto *event = new Json{{"type", type}, {"body", body}, {"generation", active}};
        if (!PostMessageW(target, MEDIA_EVENT, 0, reinterpret_cast<LPARAM>(event)))
            delete event;
    }
    void open_profiles() {
        PWSTR path = nullptr;
        if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_LocalAppData, 0, nullptr, &path))) {
            auto file = std::filesystem::path(path) / L"LanCast" / L"receiver-profiles.json";
            CoTaskMemFree(path);
            command("profiles.open", {{"path", utf8(file.wstring())}});
        }
    }
    size_t source_index() {
        auto index = SendMessageW(windows_list, CB_GETCURSEL, 0, 0);
        return index >= 0 && static_cast<size_t>(index) < windows.size()
                   ? static_cast<size_t>(index)
                   : 0;
    }
    void begin_live(bool synthetic) {
        if (busy())
            throw std::runtime_error("先停止当前分享");
        select_route(dlna_ip);
        probe = synthetic;
        probe_prompted = false;
        live_pending = true;
        ++generation;
        mode = "dlna_live";
        selected_audio = SendMessageW(audio_check, BM_GETCHECK, 0, 0) == BST_CHECKED;
        command("live.create", {{"address", local_address + ":0"},
                                {"allowedIp", dlna_ip},
                                {"deviceId", dlna_id},
                                {"synthetic", synthetic},
                                {"audio", selected_audio},
                                {"generation", generation}});
        status(synthetic ? "正在发送合成测试画面和提示音，没有采集屏幕" : "正在准备 DLNA 直播");
    }
    void event(const Json &e) {
        const auto type = e.value("type", "");
        const auto body = e.value("body", Json::object());
        if (type.starts_with("live.") && body.contains("generation") &&
            body.value("generation", uint64_t{0}) != generation)
            return;
        if (type == "connected") {
            connected = true;
            pairing = false;
            status("安全连接已建立");
        } else if (type == "pair.waiting")
            status("请在电视上确认此设备");
        else if (type == "disconnected") {
            connected = false;
            pairing = false;
            ++generation;
            live_pending = false;
            profile_pending = false;
            pending_session.clear();
            file_pending = false;
            media.reset();
            session.clear();
            shared_file = nullptr;
            if (!stopping) {
                command("stop");
                stopping = true;
            }
            status("连接已断开，分享已停止；重新连接需要电视确认");
        } else if (type == "error" || type == "live.failed") {
            pairing = false;
            file_pending = false;
            pending_session.clear();
            profile_pending = false;
            if (media || live_pending || !shared_file.is_null())
                stop();
            status("操作未完成：" + body.value("code", "ERROR") + "。请检查电视和网络后重试。");
        } else if (type == "live.created") {
            if (!live_pending || body.value("generation", uint64_t{0}) != generation)
                return;
            const auto active = generation;
            const auto handle = core;
            auto index = source_index();
            media =
                std::make_unique<MediaSender>([target = main_window, active](auto type, auto body) {
                    post_media(target, active, type, body);
                });
            media->start_live(windows[index], monitors[index], selected_audio, probe,
                              [handle](const uint8_t *bytes, size_t length) {
                                  return lancast_write_ts(handle, bytes, length);
                              });
            command("dlna.load", {{"deviceId", dlna_id}, {"url", body.at("url")}, {"live", true}});
            live_pending = false;
        } else if (type == "profile.checked") {
            if (!profile_pending || body.value("deviceId", "") != dlna_id ||
                body.value("generation", uint64_t{0}) != generation)
                return;
            profile_pending = false;
            if (body.value("passed", false))
                begin_live(false);
            else
                status("请先点击“测试电视兼容性”，确认画面和声音正常后再投屏");
        } else if (type == "probe.saved") {
            stop();
            status(body.value("passed", false) ? "测试档案已保存，可点击分享屏幕开始真实内容分享"
                                               : "当前配置未通过，可继续使用 MP4 文件播放");
        } else if (type == "live.state") {
            auto state = body.value("state", "");
            status(state == "pulling"      ? "电视正在接收直播，实际显示与延迟请以电视为准"
                   : state == "recovering" ? "电视拉流中断，正在进行一次恢复"
                                           : "等待电视拉流或测试确认");
            if (probe && !probe_prompted && state == "awaiting_user_confirmation") {
                probe_prompted = true;
                auto result = MessageBoxW(
                    main_window,
                    L"电视是否持续显示红、绿、蓝交替画面？\n有声档还需确认听到提示音。\n\n"
                    L"请选择“是”保存通过记录，“否”记录不兼容，“取消”结束测试。",
                    L"确认电视测试结果", MB_YESNOCANCEL | MB_ICONQUESTION);
                if (result == IDCANCEL)
                    stop();
                else
                    command("probe.confirm", {{"passed", result == IDYES}});
            }
        } else if (type == "devices" || type == "dlna.devices") {
            if (!scanning || body.value("scanGeneration", uint64_t{0}) != scan_generation)
                return;
            catalog.update(body.value("devices", Json::array()), type == "dlna.devices");
            if (type == "devices")
                scan_lancast_pending = false;
            else
                scan_dlna_pending = false;
            scanning = scan_lancast_pending || scan_dlna_pending;
            refresh_devices();
            if (!scanning && selected_key.empty() && catalog.entries().size() == 1) {
                SendMessageW(devices_list, CB_SETCURSEL, 0, 0);
                select_device();
            } else if (!scanning)
                status(
                    catalog.entries().empty()
                        ? "未发现电视。请确认各端使用当前版本并连接同一网络；高级设置可切换网卡。"
                        : "请选择要连接的电视");
        } else if (type == "file.shared") {
            if (!file_pending || stopping || exiting)
                return;
            file_pending = false;
            shared_file = body;
            if (!dlna_id.empty())
                command("dlna.load",
                        {{"deviceId", dlna_id}, {"url", body["url"]}, {"title", "LanCast Video"}});
            else {
                mode = "file";
                pending_session =
                    send("session.start", {{"mode", "file"}, {"audioRequested", true}});
            }
        } else if (type == "stopped") {
            stopping = false;
            status("已停止分享。可重新连接电视或播放视频。");
            if (exiting)
                destroy();
        } else if (type == "dlna.state")
            status("DLNA 指令已返回；是否播放请以电视为准");
        else if (type == "message") {
            const auto kind = body.value("type", "");
            const auto data = body.value("body", Json::object());
            if (kind == "session.accepted") {
                if (pending_session.empty() || body.value("replyTo", "") != pending_session)
                    return;
                pending_session.clear();
                session = body.at("sessionId").get<std::string>();
                if (mode == "mirror") {
                    const auto active = ++generation;
                    media = std::make_unique<MediaSender>(
                        [target = main_window, active](auto type, auto body) {
                            post_media(target, active, type, body);
                        });
                    auto index = SendMessageW(windows_list, CB_GETCURSEL, 0, 0);
                    HWND selected = index > 0 && static_cast<size_t>(index) < windows.size()
                                        ? windows[static_cast<size_t>(index)]
                                        : nullptr;
                    auto profile = data.at("selectedProfile");
                    profile["width"] = std::min(profile.value("width", 1280), 1280);
                    profile["height"] = std::min(profile.value("height", 720), 720);
                    profile["fps"] = std::min(profile.value("fps", 30), 30);
                    profile["monitor"] = reinterpret_cast<uintptr_t>(monitors[source_index()]);
                    media->start(selected,
                                 SendMessageW(audio_check, BM_GETCHECK, 0, 0) == BST_CHECKED,
                                 profile);
                    status("正在分享屏幕，可最小化到托盘继续投屏");
                } else {
                    shared_file["mediaId"] = uuid();
                    shared_file["durationMs"] = nullptr;
                    send("file.load", shared_file);
                }
            } else if (kind != "error" && body.value("sessionId", "") != session)
                return;
            else if (kind == "rtc.answer" && media)
                media->answer(data.at("sdp"), data.at("negotiationId"));
            else if (kind == "rtc.ice" && media)
                media->ice(data);
            else if (kind == "rtc.restart" && media) {
                media->restart(data);
                status("媒体连接中断，正在恢复…");
            } else if (kind == "session.stop") {
                stop();
                status(data.value("reason", "") == "RTC_RECOVERY_EXHAUSTED"
                           ? "媒体恢复超时，请重新分享"
                           : "接收端已停止");
            } else if (kind == "error") {
                stop();
                status(data.value("code", "ERROR"));
            } else if (kind == "session.state")
                status("接收端已报告播放就绪");
        }
        update_controls();
    }
    void refresh_windows() {
        windows = {nullptr};
        monitors = {nullptr};
        SendMessageW(windows_list, CB_RESETCONTENT, 0, 0);
        SendMessageW(windows_list, CB_ADDSTRING, 0, reinterpret_cast<LPARAM>(L"整个主屏幕"));
        EnumDisplayMonitors(
            nullptr, nullptr,
            +[](HMONITOR monitor, HDC, LPRECT, LPARAM context) -> BOOL {
                auto &self = *reinterpret_cast<DesktopApp *>(context);
                MONITORINFOEXW info{};
                info.cbSize = sizeof(info);
                GetMonitorInfoW(monitor, &info);
                if (info.dwFlags & MONITORINFOF_PRIMARY) {
                    self.monitors[0] = monitor;
                    return TRUE;
                }
                const auto label = L"扩展屏幕 " + std::to_wstring(self.monitors.size());
                self.windows.push_back(nullptr);
                self.monitors.push_back(monitor);
                SendMessageW(self.windows_list, CB_ADDSTRING, 0,
                             reinterpret_cast<LPARAM>(label.c_str()));
                return TRUE;
            },
            reinterpret_cast<LPARAM>(this));
        EnumWindows(
            +[](HWND window, LPARAM context) -> BOOL {
                auto &self = *reinterpret_cast<DesktopApp *>(context);
                if (window == self.main_window || !IsWindowVisible(window) ||
                    GetWindowTextLengthW(window) == 0)
                    return TRUE;
                wchar_t title[256];
                GetWindowTextW(window, title, 256);
                self.windows.push_back(window);
                self.monitors.push_back(nullptr);
                SendMessageW(self.windows_list, CB_ADDSTRING, 0, reinterpret_cast<LPARAM>(title));
                return TRUE;
            },
            reinterpret_cast<LPARAM>(this));
        SendMessageW(windows_list, CB_SETCURSEL, 0, 0);
    }
    void click(int id) {
        switch (id) {
        case 1:
            scan();
            break;
        case 4: {
            if (connected || pairing || busy())
                return;
            auto endpoint = receiver_address;
            auto split = endpoint.rfind(':');
            if (split == std::string::npos || !network::lan_ipv4(endpoint.substr(0, split))) {
                throw std::runtime_error("请刷新并选择当前版本的接收端");
            }
            const auto port = endpoint.substr(split + 1);
            if (port.empty() || port.size() > 5 ||
                !std::all_of(port.begin(), port.end(),
                             [](char c) { return c >= '0' && c <= '9'; }) ||
                std::stoi(port) < 1 || std::stoi(port) > 65535)
                throw std::runtime_error("电视端口必须是 1–65535 之间的数字");
            const auto pin = receiver_fingerprint;
            if (!valid_fingerprint(pin)) {
                throw std::runtime_error("接收端身份信息无效，请更新各端并重新搜索");
            }
            auto invite = text(invitation);
            std::erase_if(invite, [](char c) { return c == ' ' || c == '\r' || c == '\n'; });
            if (invite.size() != 8 || !std::all_of(invite.begin(), invite.end(),
                                                   [](char c) { return c >= '0' && c <= '9'; })) {
                SetFocus(invitation);
                throw std::runtime_error("请输入电视上的 8 位数字配对码；过期时请在电视刷新");
            }
            std::string formatted;
            for (size_t i = 0; i < pin.size(); i += 8) {
                formatted += pin.substr(i, 8);
                formatted += (i == 24 ? "\n" : "  ");
            }
            const auto prompt = "请确认以下完整指纹与电视显示的一致：\n\n" + formatted +
                                "\n\n地址：" + endpoint +
                                "\n自动发现的信息尚未验证。只有全部一致时才继续。";
            if (MessageBoxW(main_window, wide(prompt).c_str(), L"核对电视身份",
                            MB_OKCANCEL | MB_ICONINFORMATION | MB_DEFBUTTON2) != IDOK)
                return;
            select_route(endpoint.substr(0, split));
            command("connect", {{"address", endpoint},
                                {"fingerprint", pin},
                                {"invite", invite},
                                {"name", "LanCast Windows"}});
            dlna_id.clear();
            dlna_ip.clear();
            pairing = true;
            SetWindowTextW(invitation, L"");
            status("正在连接电视，请留意电视上的允许连接提示…");
            break;
        }
        case 5:
            if (!media_available)
                throw std::runtime_error("原生媒体后端加载失败");
            if (busy())
                throw std::runtime_error("先停止当前分享");
            if (!dlna_id.empty()) {
                profile_pending = true;
                ++generation;
                command("profile.check",
                        {{"deviceId", dlna_id},
                         {"audio", SendMessageW(audio_check, BM_GETCHECK, 0, 0) == BST_CHECKED},
                         {"generation", generation}});
                status("正在检查这台电视的兼容性记录…");
                break;
            }
            if (!connected)
                throw std::runtime_error("分享屏幕前请连接自有接收端");
            mode = "mirror";
            pending_session = send(
                "session.start",
                {{"mode", mode},
                 {"rtcRecovery", "replace-v1"},
                 {"audioRequested", SendMessageW(audio_check, BM_GETCHECK, 0, 0) == BST_CHECKED}});
            status("正在准备屏幕分享…");
            break;
        case 6: {
            if (busy())
                throw std::runtime_error("请先停止当前分享");
            if (!connected && dlna_id.empty())
                throw std::runtime_error("先连接自有接收端或选择 DLNA 电视");
            wchar_t path[32768] = {0};
            OPENFILENAMEW dialog{sizeof(dialog)};
            dialog.hwndOwner = main_window;
            dialog.lpstrFilter = L"MP4 视频\0*.mp4\0";
            dialog.lpstrFile = path;
            dialog.nMaxFile = 32768;
            dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
            if (GetOpenFileNameW(&dialog)) {
                auto remote = receiver_address;
                auto ip = dlna_id.empty() ? remote.substr(0, remote.rfind(':')) : dlna_ip;
                select_route(ip);
                mode = "file";
                file_pending = true;
                command("file.share", {{"path", utf8(path)},
                                       {"address", local_address + ":0"},
                                       {"allowedIp", ip},
                                       {"encrypted", dlna_id.empty()}});
                status("正在准备视频文件…");
            }
            break;
        }
        case 7:
        case 8:
        case 9: {
            if (mode != "file" || shared_file.is_null())
                throw std::runtime_error("直播只支持停止，不能暂停或跳转");
            const char *action = id == 7 ? "pause" : id == 8 ? "play" : "seek";
            if (!dlna_id.empty())
                command("dlna.command",
                        {{"deviceId", dlna_id}, {"action", action}, {"positionMs", 60000}});
            else
                send("playback.command", {{"action", action}, {"positionMs", 60000}});
            break;
        }
        case 10:
            stop();
            break;
        case 11: {
            const bool reset_core = pairing;
            if (reset_core) {
                release_core(core);
                core = create_core();
                open_profiles();
                pairing = false;
                stopping = false;
                connected = false;
            } else
                stop();
            dlna_id.clear();
            dlna_ip.clear();
            selected_key.clear();
            SendMessageW(devices_list, CB_SETCURSEL, static_cast<WPARAM>(-1), 0);
            receiver_address.clear();
            receiver_fingerprint.clear();
            SetWindowTextW(invitation, L"");
            if (reset_core) {
                scanning = false;
                catalog.clear();
                refresh_devices();
                scan();
            }
            status("已断开；新连接需要重新选择电视并确认");
            break;
        }
        case 12:
            MessageBoxW(
                main_window,
                L"无法安装 App 的电视：\n• DLNA 可播放 MP4；合成测试通过后可尝试兼容直播。\n• "
                L"已有 Miracast 可使用 Windows Win+K。\n• Apple 设备需要电视已有 AirPlay "
                L"才能使用系统屏幕镜像。\n• 无共同协议时请外接允许安装 LanCast 的 Android HDMI "
                L"盒子。\n\n系统投屏不属于 LanCast 媒体会话。Apple TV 不能安装 Android APK。",
                L"系统投屏指引", MB_OK);
            break;
        case 13:
            refresh_windows();
            break;
        case 14:
            if (dlna_id.empty())
                throw std::runtime_error("先选择 DLNA 电视");
            begin_live(true);
            break;
        case 15:
            view.toggle_advanced();
            break;
        case 17:
            manual_network = false;
            refresh_network();
            scan();
            break;
        case TrayIcon::restore:
            tray.show();
            break;
        case TrayIcon::stop:
            if (pairing)
                click(11);
            else
                stop();
            break;
        case TrayIcon::quit:
            close();
            break;
        }
    }
    static LRESULT CALLBACK procedure(HWND window, UINT message, WPARAM w, LPARAM l) {
        auto *self = reinterpret_cast<DesktopApp *>(GetWindowLongPtrW(window, GWLP_USERDATA));
        if (message == WM_NCCREATE) {
            self = static_cast<DesktopApp *>(reinterpret_cast<CREATESTRUCTW *>(l)->lpCreateParams);
            self->main_window = window;
            SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
        }
        return self ? self->dispatch(window, message, w, l) : DefWindowProcW(window, message, w, l);
    }
    LRESULT dispatch(HWND window, UINT message, WPARAM w, LPARAM l) {
        try {
            if (tray.taskbar_created() && message == tray.taskbar_created()) {
                tray.recreate();
                return 0;
            }
            if (message == TrayIcon::message) {
                tray.event(l);
                return 0;
            }
            LRESULT handled{};
            if (ready && view.handle(message, w, l, handled))
                return handled;
            switch (message) {
            case WM_SIZE:
                if (w == SIZE_MINIMIZED && ready)
                    tray.hide();
                break;
            case WM_COMMAND:
                if (!ready || exiting)
                    return 0;
                if (HIWORD(w) == CBN_SELCHANGE && LOWORD(w) == MainView::devices) {
                    select_device();
                    return 0;
                }
                if (HIWORD(w) == CBN_SELCHANGE && LOWORD(w) == 16) {
                    choose_network();
                    return 0;
                }
                if (HIWORD(w) == BN_CLICKED)
                    click(LOWORD(w));
                return 0;
            case WM_TIMER:
                if (!core)
                    return 0;
                if (exiting && GetTickCount64() - exit_started > 3000) {
                    destroy();
                    return 0;
                }
                if (scanning && GetTickCount64() - scan_started > 25000) {
                    scanning = false;
                    scan_lancast_pending = scan_dlna_pending = false;
                    status("搜索结束。未找到电视时，请检查同一网络或使用高级设置。");
                }
                for (int i = 0; i < 32; ++i) {
                    auto buffer = lancast_poll(core);
                    if (!buffer.data)
                        break;
                    std::string raw(reinterpret_cast<char *>(buffer.data), buffer.len);
                    lancast_free_buffer(buffer);
                    event(Json::parse(raw));
                    if (!core)
                        break;
                }
                return 0;
            case MEDIA_EVENT: {
                std::unique_ptr<Json> e(reinterpret_cast<Json *>(l));
                if (e->value("generation", uint64_t{0}) != generation)
                    return 0;
                auto kind = e->at("type").get<std::string>();
                if (kind == "media.error" || kind == "media.ended") {
                    stop();
                    status((*e)["body"].value("code", "媒体已结束"));
                } else if (kind == "media.connected")
                    status("WebRTC 媒体通道已建立");
                else if (!session.empty())
                    send(kind, e->at("body"));
                return 0;
            }
            case WM_CLOSE:
                close();
                return 0;
            case WM_DESTROY:
                KillTimer(window, 1);
                PostQuitMessage(0);
                return 0;
            }
        } catch (const std::exception &error) {
            if (message == WM_TIMER || message == MEDIA_EVENT ||
                (!stopping && (file_pending || profile_pending || live_pending)))
                stop();
            status(error.what());
        }
        return DefWindowProcW(window, message, w, l);
    }

    MainView view;
    TrayIcon tray;
    DeviceCatalog catalog;
    std::vector<network::Interface> interfaces;
    std::string selected_key;
    bool ready = false, pairing = false, file_pending = false, stopping = false;
    bool scanning = false, scan_lancast_pending = false, scan_dlna_pending = false;
    bool manual_network = false, media_available = false, exiting = false;
    ULONGLONG scan_started = 0, exit_started = 0;
    uint64_t scan_generation = 0;
    bool busy() const {
        return media || live_pending || profile_pending || file_pending ||
               !pending_session.empty() || !shared_file.is_null() || !session.empty() || stopping;
    }
    void update_controls() {
        if (!ready)
            return;
        const bool active = busy();
        for (int id : {1, 16, 17, MainView::devices, MainView::invitation})
            EnableWindow(view.get(id), !active && !connected && !pairing && !scanning && !exiting);
        EnableWindow(view.get(4), !active && !connected && !pairing && !scanning && !exiting &&
                                      !receiver_address.empty() && dlna_id.empty());
        EnableWindow(view.get(5), !active && !pairing && !scanning && !exiting && media_available &&
                                      (connected || !dlna_id.empty()));
        EnableWindow(view.get(6), !active && !pairing && !scanning && !exiting &&
                                      (connected || !dlna_id.empty()));
        EnableWindow(view.get(14), !active && !pairing && !scanning && !exiting &&
                                       media_available && !dlna_id.empty());
        for (int id : {7, 8, 9})
            EnableWindow(view.get(id),
                         mode == "file" && !shared_file.is_null() && !stopping && !exiting);
        EnableWindow(view.get(10), active && !stopping && !exiting);
        EnableWindow(view.get(11),
                     !exiting && (connected || pairing || active || !dlna_id.empty()));
        for (int id : {MainView::sources, MainView::audio, 13})
            EnableWindow(view.get(id), !active && !exiting);
        SetWindowTextW(view.get(1), scanning ? L"搜索中…" : L"刷新电视");
        SetWindowTextW(view.get(4), pairing     ? L"等待电视确认"
                                    : connected ? L"已连接"
                                                : L"连接电视");
        EnableWindow(invitation, !active && !connected && !pairing && !scanning && dlna_id.empty());
    }
    void refresh_network() {
        interfaces = network::interfaces();
        auto combo = view.get(16);
        SendMessageW(combo, CB_RESETCONTENT, 0, 0);
        for (const auto &item : interfaces) {
            auto label = item.name + L" · " + wide(item.ip);
            SendMessageW(combo, CB_ADDSTRING, 0, reinterpret_cast<LPARAM>(label.c_str()));
        }
        SendMessageW(combo, CB_SETCURSEL, interfaces.empty() ? -1 : 0, 0);
        local_address = interfaces.empty() ? "" : interfaces.front().ip;
        network_label();
    }
    void network_label() {
        auto ip = local_address;
        auto label = ip.empty()
                         ? L"未连接局域网 · 请先连接与电视相同的 Wi-Fi 或网线"
                         : (manual_network ? L"手动选择网络 · " : L"已自动选择网络 · ") + wide(ip);
        SetWindowTextW(view.get(MainView::network_summary), label.c_str());
    }
    void choose_network() {
        auto index = SendMessageW(view.get(16), CB_GETCURSEL, 0, 0);
        if (index < 0 || static_cast<size_t>(index) >= interfaces.size())
            return;
        manual_network = true;
        local_address = interfaces[static_cast<size_t>(index)].ip;
        network_label();
        scan();
    }
    void select_route(const std::string &destination) {
        if (!manual_network) {
            interfaces = network::interfaces();
            const auto ip = network::source_for(destination, interfaces);
            if (ip.empty())
                throw std::runtime_error(
                    "没有到这台电视的局域网路由，请检查网络或在高级设置中选择网卡");
            local_address = ip;
            // Keep the advanced selector in sync with route selection.
            auto combo = view.get(16);
            SendMessageW(combo, CB_RESETCONTENT, 0, 0);
            for (size_t i = 0; i < interfaces.size(); ++i) {
                auto label = interfaces[i].name + L" · " + wide(interfaces[i].ip);
                SendMessageW(combo, CB_ADDSTRING, 0, reinterpret_cast<LPARAM>(label.c_str()));
                if (interfaces[i].ip == ip)
                    SendMessageW(combo, CB_SETCURSEL, i, 0);
            }
        } else {
            auto current = network::interfaces();
            if (std::none_of(current.begin(), current.end(),
                             [&](const auto &i) { return i.ip == local_address; }))
                throw std::runtime_error("所选网卡已断开，请重新自动选择网络");
        }
        if (local_address.empty())
            throw std::runtime_error("请先连接本地网络");
        network_label();
    }
    void scan() {
        if (busy() || pairing || connected || scanning)
            return;
        if (!manual_network)
            refresh_network();
        catalog.clear();
        selected_key.clear();
        dlna_id.clear();
        dlna_ip.clear();
        receiver_address.clear();
        receiver_fingerprint.clear();
        SetWindowTextW(invitation, L"");
        SendMessageW(devices_list, CB_RESETCONTENT, 0, 0);
        scanning = scan_lancast_pending = true;
        scan_dlna_pending = !local_address.empty();
        scan_started = GetTickCount64();
        ++scan_generation;
        command("scan", {{"scanGeneration", scan_generation}});
        if (scan_dlna_pending)
            command("dlna.scan",
                    {{"interface", local_address}, {"scanGeneration", scan_generation}});
        status("正在搜索电视… 请让电视接收端保持打开，并连接同一网络。");
    }
    void refresh_devices() {
        SendMessageW(devices_list, CB_RESETCONTENT, 0, 0);
        int selected = -1;
        const auto &entries = catalog.entries();
        for (size_t i = 0; i < entries.size(); ++i) {
            const auto &d = entries[i];
            const auto label =
                wide(d.name + (d.dlna ? " · 普通电视 (DLNA) · " : " · LanCast · ") + d.ip);
            SendMessageW(devices_list, CB_ADDSTRING, 0, reinterpret_cast<LPARAM>(label.c_str()));
            if (d.key == selected_key)
                selected = static_cast<int>(i);
        }
        SendMessageW(devices_list, CB_SETCURSEL, selected, 0);
        update_controls();
    }
    void select_device() {
        if (busy() || connected || pairing)
            return;
        auto index = SendMessageW(devices_list, CB_GETCURSEL, 0, 0);
        if (index < 0 || static_cast<size_t>(index) >= catalog.entries().size())
            return;
        const auto &d = catalog.entries()[static_cast<size_t>(index)];
        selected_key = d.key;
        dlna_id = d.dlna ? d.id : "";
        dlna_ip = d.dlna ? d.ip : "";
        receiver_address = d.address;
        receiver_fingerprint = d.fingerprint;
        SetWindowTextW(invitation, L"");
        select_route(d.ip);
        status(d.dlna ? "已选择普通电视。可直接播放 "
                        "MP4；分享屏幕前请先测试电视兼容性。视频通过本地网络明文传输。"
                      : "已自动获取电视地址和指纹。输入电视上的配对码，点击连接电视。");
    }
    void close() {
        if (exiting)
            return;
        if ((busy() || pairing) &&
            MessageBoxW(main_window,
                        L"退出将停止当前分享。要继续在后台投屏，请选择取消后最小化到托盘。",
                        L"退出 LanCast？", MB_OKCANCEL | MB_ICONQUESTION | MB_DEFBUTTON2) != IDOK)
            return;
        tray.show();
        exiting = true;
        exit_started = GetTickCount64();
        if (pairing) {
            destroy();
            return;
        } // shutdown interrupts an in-flight handshake.
        stop();
    }
    void destroy() {
        ++generation;
        media.reset();
        tray.remove();
        if (core) {
            release_core(core);
            core = 0;
        }
        DestroyWindow(main_window);
    }

  public:
    int run(HINSTANCE instance, int show) {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
        INITCOMMONCONTROLSEX controls{sizeof(controls), ICC_STANDARD_CLASSES};
        InitCommonControlsEx(&controls);
        core = create_core();
        if (!core) {
            MessageBoxW(nullptr, L"控制核心启动失败", L"LanCast", MB_OK);
            CoUninitialize();
            return 1;
        }
        open_profiles();
        WNDCLASSW klass{};
        klass.hInstance = instance;
        klass.lpfnWndProc = procedure;
        klass.lpszClassName = L"LanCastWindow";
        klass.hCursor = LoadCursor(nullptr, IDC_ARROW);
        klass.hIcon = LoadIconW(instance, MAKEINTRESOURCEW(1));
        RegisterClassW(&klass);
        const auto dpi = GetDpiForSystem();
        RECT bounds{0, 0, MulDiv(840, dpi, 96), MulDiv(686, dpi, 96)};
        AdjustWindowRectExForDpi(&bounds, WS_OVERLAPPEDWINDOW | WS_VSCROLL, FALSE, 0, dpi);
        RECT work{};
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &work, 0);
        main_window = CreateWindowW(klass.lpszClassName, L"LanCast · 局域网投屏",
                                    WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_CLIPCHILDREN,
                                    CW_USEDEFAULT, CW_USEDEFAULT, bounds.right - bounds.left,
                                    std::min(bounds.bottom - bounds.top, work.bottom - work.top),
                                    nullptr, nullptr, instance, this);
        if (!main_window) {
            release_core(core);
            CoUninitialize();
            return 1;
        }
        SendMessageW(main_window, WM_SETICON, ICON_BIG, reinterpret_cast<LPARAM>(klass.hIcon));
        SendMessageW(main_window, WM_SETICON, ICON_SMALL,
                     reinterpret_cast<LPARAM>(LoadImageW(
                         instance, MAKEINTRESOURCEW(1), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON),
                         GetSystemMetrics(SM_CYSMICON), LR_SHARED)));
        view.create(main_window);
        status_text = view.get(MainView::status);
        invitation = view.get(MainView::invitation);
        windows_list = view.get(MainView::sources);
        audio_check = view.get(MainView::audio);
        devices_list = view.get(MainView::devices);
        tray.attach(main_window, klass.hIcon);
        media_available = MediaSender::available();
        ready = true;
        refresh_windows();
        SetTimer(main_window, 1, 50, nullptr);
        ShowWindow(main_window, show);
        try {
            scan();
        } catch (const std::exception &error) {
            status(error.what());
        }
        if (!media_available)
            status("媒体库加载失败，屏幕分享不可用。请完整解压程序和三个 DLL；文件播放仍可用。");
        MSG msg{};
        while (GetMessageW(&msg, nullptr, 0, 0) > 0) {
            if (msg.message == WM_KEYDOWN && msg.wParam == VK_RETURN && msg.hwnd == invitation) {
                if (IsWindowEnabled(view.get(4))) {
                    try {
                        click(4);
                    } catch (const std::exception &error) {
                        status(error.what());
                    }
                }
                continue;
            }
            if (!IsDialogMessageW(main_window, &msg)) {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // Dispose already queued media events after the window has been destroyed.
        while (PeekMessageW(&msg, nullptr, MEDIA_EVENT, MEDIA_EVENT, PM_REMOVE))
            delete reinterpret_cast<Json *>(msg.lParam);
        CoUninitialize();
        return 0;
    }
};
} // namespace
int run_desktop(HINSTANCE instance, int show) {
    DesktopApp app{};
    return app.run(instance, show);
}
