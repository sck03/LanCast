#pragma once
#include "lancast_media.h"
#include "types.h"
namespace lancast {
class TsOutput {
  public:
    TsOutput(const CaptureConfig &config, LcTsWrite write, void *user);
    ~TsOutput();
    void send(const Packet &packet);

  private:
    HMODULE module_ = nullptr;
    LcTsMux *mux_ = nullptr;
    decltype(&lc_ts_video) video_ = nullptr;
    decltype(&lc_ts_audio) audio_ = nullptr;
    decltype(&lc_ts_destroy) destroy_ = nullptr;
};
} // namespace lancast
