These H.264 fixtures are the first configuration and IDR NAL from the existing
LanCast RX580 synthetic encoder probe (`.cache/rx580-bt709.h264`, D08/D10).
They contain generated test imagery, not captured user screen content.
The socket test encrypts the access unit, sends it through the real receiver and
checks the decrypted Annex B payload. It does not claim Android decoder or TV validation.
