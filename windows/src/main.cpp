#include <windows.h>
#include <commdlg.h>
#include <commctrl.h>
#include <gst/gst.h>
#include <nlohmann/json.hpp>
#include "lancast.h"
#include "media.h"
#include <memory>
#include <vector>
#include <string>
#include <stdexcept>
using Json=nlohmann::json;
static constexpr UINT MEDIA_EVENT=WM_APP+1;
static HWND main_window, status_text, local_ip, address, fingerprint, invitation, windows_list, audio_check, devices_list;
static LancastHandle core=0;
static std::unique_ptr<MediaSender> media;
static std::string session, mode, dlna_id, dlna_ip;
static Json shared_file, devices=Json::array();
static bool connected=false;
static std::vector<HWND> windows;
static std::wstring wide(const std::string& text) {if(text.empty())return {};int n=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text.data(),static_cast<int>(text.size()),nullptr,0);std::wstring result(n,0);MultiByteToWideChar(CP_UTF8,0,text.data(),static_cast<int>(text.size()),result.data(),n);return result;}
static std::string utf8(const std::wstring& text) {int n=WideCharToMultiByte(CP_UTF8,0,text.data(),static_cast<int>(text.size()),nullptr,0,nullptr,nullptr);std::string result(n,0);WideCharToMultiByte(CP_UTF8,0,text.data(),static_cast<int>(text.size()),result.data(),n,nullptr,nullptr);return result;}
static std::string text(HWND control) {const int n=GetWindowTextLengthW(control);std::wstring value(n+1,0);GetWindowTextW(control,value.data(),n+1);value.resize(n);return utf8(value);}
static void status(const std::string& value){SetWindowTextW(status_text,wide(value).c_str());}
static void command(const std::string& op,Json body=Json::object()){body["op"]=op;auto s=body.dump();if(lancast_command(core,reinterpret_cast<const uint8_t*>(s.data()),s.size())!=0)throw std::runtime_error("控制请求队列已满或请求无效");}
static std::string uuid(){GUID id;CoCreateGuid(&id);wchar_t buffer[40];StringFromGUID2(id,buffer,40);return utf8(std::wstring(buffer+1,36));}
static void send(const std::string& type,Json body=Json::object()){command("send",{{"message",{{"version",1},{"id",uuid()},{"type",type},{"sessionId",session.empty()?Json(nullptr):Json(session)},{"body",body}}}});}
static void stop(){if(!session.empty())send("session.stop",{{"reason","sender_stopped"}});if(!dlna_id.empty())command("dlna.command",{{"deviceId",dlna_id},{"action","stop"}});media.reset();session.clear();shared_file=nullptr;command("stop");status("已停止分享");}
static void post_media(std::string type,Json body){auto* event=new Json{{"type",type},{"body",body}};if(!PostMessageW(main_window,MEDIA_EVENT,0,reinterpret_cast<LPARAM>(event)))delete event;}
static void event(const Json& e){
    const auto type=e.value("type","");const auto body=e.value("body",Json::object());
    if(type=="connected"){connected=true;status("安全连接已建立");}
    else if(type=="pair.waiting")status("请在电视上确认此设备");
    else if(type=="disconnected"){connected=false;media.reset();session.clear();status("连接已断开，采集已停止；请断开并重新配对");}
    else if(type=="error")status(body.value("code","ERROR"));
    else if(type=="devices" || type=="dlna.devices"){
        devices=body.at("devices");SendMessageW(devices_list,CB_RESETCONTENT,0,0);
        for(const auto& d:devices)SendMessageW(devices_list,CB_ADDSTRING,0,reinterpret_cast<LPARAM>(wide(d.value("name","TV")).c_str()));
        SendMessageW(devices_list,CB_SETCURSEL,0,0);status(devices.empty()?"未发现设备，可手动连接自有接收端":"请选择设备，然后按“使用设备”");
    } else if(type=="file.shared"){
        shared_file=body;
        if(!dlna_id.empty())command("dlna.load",{{"deviceId",dlna_id},{"url",body["url"]},{"title","LanCast Video"}});
        else {mode="file";send("session.start",{{"mode","file"},{"audioRequested",true}});}
    } else if(type=="dlna.state")status("DLNA 指令已返回；是否播放请以电视为准");
    else if(type=="message"){
        const auto kind=body.value("type","");const auto data=body.value("body",Json::object());
        if(kind=="session.accepted"){
            session=body.at("sessionId").get<std::string>();
            if(mode=="mirror"){
                media=std::make_unique<MediaSender>(post_media);
                auto index=SendMessageW(windows_list,CB_GETCURSEL,0,0);
                HWND selected=index>0&&static_cast<size_t>(index)<windows.size()?windows[static_cast<size_t>(index)]:nullptr;
                media->start(selected,SendMessageW(audio_check,BM_GETCHECK,0,0)==BST_CHECKED,data.at("selectedProfile"));
                status("已启动 WGC / 硬件 H.264 / WebRTC；窗口画面配系统声音");
            }else {shared_file["mediaId"]=uuid();shared_file["durationMs"]=nullptr;send("file.load",shared_file);}
        }else if(kind=="rtc.answer"&&media)media->answer(data.at("sdp"),data.at("negotiationId"));
        else if(kind=="rtc.ice"&&media)media->ice(data);
        else if(kind=="session.stop"){media.reset();session.clear();command("stop");status("接收端已停止");}
        else if(kind=="error"){media.reset();status(data.value("code","ERROR"));}
        else if(kind=="session.state")status("接收端已报告播放就绪");
    }
}
static HWND control(const wchar_t* klass,const wchar_t* label,DWORD style,int x,int y,int w,int h,int id){return CreateWindowW(klass,label,WS_CHILD|WS_VISIBLE|style,x,y,w,h,main_window,reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)),nullptr,nullptr);}
static void refresh_windows(){windows={nullptr};SendMessageW(windows_list,CB_RESETCONTENT,0,0);SendMessageW(windows_list,CB_ADDSTRING,0,reinterpret_cast<LPARAM>(L"整个主屏幕"));EnumWindows(+[](HWND window,LPARAM)->BOOL{if(window==main_window||!IsWindowVisible(window)||GetWindowTextLengthW(window)==0)return TRUE;wchar_t title[256];GetWindowTextW(window,title,256);windows.push_back(window);SendMessageW(windows_list,CB_ADDSTRING,0,reinterpret_cast<LPARAM>(title));return TRUE;},0);SendMessageW(windows_list,CB_SETCURSEL,0,0);}
static void click(int id){
    switch(id){
    case 1:command("scan");break;
    case 2:command("dlna.scan",{{"interface",text(local_ip)}});status("扫描 DLNA 中…");break;
    case 3:{auto index=SendMessageW(devices_list,CB_GETCURSEL,0,0);if(index<0||static_cast<size_t>(index)>=devices.size())break;auto d=devices[static_cast<size_t>(index)];if(d.contains("transport")){dlna_id=d["id"];dlna_ip=d["ip"];status("已选 DLNA 设备：仅视频，文件经 LAN HTTP 明文传输");}else{dlna_id.clear();dlna_ip.clear();auto ip=d["addresses"][0].get<std::string>();SetWindowTextW(address,wide(ip+":"+std::to_string(d["port"].get<int>())).c_str());status("请核对电视显示的完整指纹和邀请");}break;}
    case 4:dlna_id.clear();dlna_ip.clear();command("connect",{{"address",text(address)},{"fingerprint",text(fingerprint)},{"invite",text(invitation)},{"name","LanCast Windows"}});break;
    case 5:if(!connected||!dlna_id.empty())throw std::runtime_error("分享屏幕前请连接自有接收端");mode="mirror";send("session.start",{{"mode",mode},{"audioRequested",SendMessageW(audio_check,BM_GETCHECK,0,0)==BST_CHECKED}});break;
    case 6:{
        if(!connected&&dlna_id.empty())throw std::runtime_error("先连接自有接收端或选择 DLNA 电视");
        wchar_t path[32768]={0};OPENFILENAMEW dialog{sizeof(dialog)};dialog.hwndOwner=main_window;dialog.lpstrFilter=L"MP4 视频\0*.mp4\0";dialog.lpstrFile=path;dialog.nMaxFile=32768;dialog.Flags=OFN_FILEMUSTEXIST|OFN_PATHMUSTEXIST;
        if(GetOpenFileNameW(&dialog)){auto remote=text(address);auto ip=dlna_id.empty()?remote.substr(0,remote.rfind(':')):dlna_ip;command("file.share",{{"path",utf8(path)},{"address",text(local_ip)+":0"},{"allowedIp",ip},{"encrypted",dlna_id.empty()}});}break;
    }
    case 7:case 8:case 9:{const char* action=id==7?"pause":id==8?"play":"seek";if(!dlna_id.empty())command("dlna.command",{{"deviceId",dlna_id},{"action",action},{"positionMs",60000}});else send("playback.command",{{"action",action},{"positionMs",60000}});break;}
    case 10:stop();break;
    case 11:stop();lancast_destroy(core);core=lancast_create();connected=false;dlna_id.clear();status("已断开；新连接需要电视确认");break;
    case 12:MessageBoxW(main_window,L"无法安装 App 的电视：\n• DLNA 只支持视频文件。\n• 已有 Miracast 可使用 Windows Win+K。\n• Apple 设备需要电视已有 AirPlay 才能使用系统屏幕镜像。\n• 无共同协议时请外接允许安装 LanCast 的 Android HDMI 盒子。\n\n系统投屏不属于 LanCast 媒体会话。Apple TV 不能安装 Android APK。",L"系统投屏指引",MB_OK);break;
    case 13:refresh_windows();break;
    }
}
static LRESULT CALLBACK procedure(HWND window,UINT message,WPARAM w,LPARAM l){
    try{
        switch(message){
        case WM_COMMAND:if(HIWORD(w)==BN_CLICKED)click(LOWORD(w));return 0;
        case WM_TIMER:for(int i=0;i<32;++i){auto buffer=lancast_poll(core);if(!buffer.data)break;std::string raw(reinterpret_cast<char*>(buffer.data),buffer.len);lancast_free_buffer(buffer);event(Json::parse(raw));}if(media)media->pump();return 0;
        case MEDIA_EVENT:{std::unique_ptr<Json> e(reinterpret_cast<Json*>(l));auto kind=e->at("type").get<std::string>();if(kind=="media.error"||kind=="media.ended"){stop();status((*e)["body"].value("code","媒体已结束"));}else if(!session.empty())send(kind,e->at("body"));return 0;}
        case WM_CLOSE:media.reset();lancast_destroy(core);core=0;DestroyWindow(window);return 0;
        case WM_DESTROY:KillTimer(window,1);PostQuitMessage(0);return 0;
        }
    }catch(const std::exception& error){media.reset();status(error.what());}
    return DefWindowProcW(window,message,w,l);
}
int WINAPI wWinMain(HINSTANCE instance,HINSTANCE,PWSTR,int show){
    CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);gst_init(nullptr,nullptr);
    core=lancast_create();if(!core){MessageBoxW(nullptr,L"控制核心启动失败",L"LanCast",MB_OK);return 1;}
    WNDCLASSW klass{};klass.hInstance=instance;klass.lpfnWndProc=procedure;klass.lpszClassName=L"LanCastWindow";klass.hCursor=LoadCursor(nullptr,IDC_ARROW);klass.hbrBackground=reinterpret_cast<HBRUSH>(COLOR_WINDOW+1);RegisterClassW(&klass);
    main_window=CreateWindowW(klass.lpszClassName,L"LanCast · 原生局域网投屏",WS_OVERLAPPEDWINDOW,CW_USEDEFAULT,CW_USEDEFAULT,880,710,nullptr,nullptr,instance,nullptr);
    control(L"STATIC",L"本机 LAN IPv4（文件服务与 DLNA 扫描）",0,20,18,400,22,0);
    local_ip=control(L"EDIT",L"",WS_BORDER|ES_AUTOHSCROLL|WS_TABSTOP,430,16,390,25,0);
    devices_list=control(L"COMBOBOX",L"",CBS_DROPDOWNLIST|WS_TABSTOP,20,58,390,220,0);
    control(L"BUTTON",L"扫描自有",WS_TABSTOP,430,55,120,30,1);control(L"BUTTON",L"扫描 DLNA",WS_TABSTOP,560,55,120,30,2);control(L"BUTTON",L"使用设备",WS_TABSTOP,690,55,130,30,3);
    control(L"STATIC",L"接收端 IPv4:8787",0,20,105,240,22,0);address=control(L"EDIT",L"",WS_BORDER|ES_AUTOHSCROLL|WS_TABSTOP,260,100,560,28,0);
    control(L"STATIC",L"电视 SHA-256 指纹（完整）",0,20,150,240,22,0);fingerprint=control(L"EDIT",L"",WS_BORDER|ES_AUTOHSCROLL|WS_TABSTOP,260,145,560,28,0);
    control(L"STATIC",L"一次性邀请（电视确认）",0,20,195,240,22,0);invitation=control(L"EDIT",L"",WS_BORDER|ES_AUTOHSCROLL|WS_TABSTOP,260,190,560,28,0);
    control(L"BUTTON",L"核对并配对",WS_TABSTOP,20,235,180,32,4);
    windows_list=control(L"COMBOBOX",L"",CBS_DROPDOWNLIST|WS_TABSTOP,20,285,650,240,0);control(L"BUTTON",L"刷新窗口",WS_TABSTOP,690,282,130,30,13);
    audio_check=control(L"BUTTON",L"系统播放声音（窗口画面不等于单进程音频）",BS_AUTOCHECKBOX|WS_TABSTOP,20,328,780,28,0);SendMessageW(audio_check,BM_SETCHECK,BST_CHECKED,0);
    control(L"BUTTON",L"分享屏幕 / 窗口",WS_TABSTOP,20,375,250,36,5);control(L"BUTTON",L"选择 MP4 播放",WS_TABSTOP,285,375,250,36,6);control(L"BUTTON",L"电视不能安装 App？",WS_TABSTOP,550,375,270,36,12);
    const wchar_t* labels[]={L"暂停",L"播放",L"跳到60秒",L"停止",L"断开连接"};for(int i=0;i<5;++i)control(L"BUTTON",labels[i],WS_TABSTOP,20+i*162,430,150,34,7+i);
    status_text=control(L"STATIC",L"准备就绪。先填写本机地址，选择接收设备并核对指纹。",SS_LEFT,20,490,800,120,0);
    refresh_windows();SetTimer(main_window,1,50,nullptr);ShowWindow(main_window,show);
    MSG msg;while(GetMessageW(&msg,nullptr,0,0)>0){if(!IsDialogMessageW(main_window,&msg)){TranslateMessage(&msg);DispatchMessageW(&msg);}}
    CoUninitialize();return 0;
}
