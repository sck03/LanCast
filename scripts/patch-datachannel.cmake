# libdatachannel 0.23.2 asks CMake for a PDB on static targets, which has no meaning.
# Keep this source-build compatibility patch explicit and idempotent.
file(READ "${SOURCE_DIR}/CMakeLists.txt" source)
string(REPLACE "if(MSVC)\n\tinstall(FILES $<TARGET_PDB_FILE:datachannel>"
               "if(MSVC AND BUILD_SHARED_LIBS)\n\tinstall(FILES $<TARGET_PDB_FILE:datachannel>" source "${source}")
file(WRITE "${SOURCE_DIR}/CMakeLists.txt" "${source}")
