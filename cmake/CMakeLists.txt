cmake_minimum_required(VERSION 3.10)

# Path to the header file containing the version definitions
set(VERSION_HEADER_FILE "../heatshrink_common.h")

# Read the header file content
file(READ "${VERSION_HEADER_FILE}" VERSION_CONTENT)

string(REGEX MATCH "HEATSHRINK_VERSION_MAJOR[ \t]+([0-9]+)" _major_match "${VERSION_CONTENT}")
set(HEATSHRINK_VERSION_MAJOR ${CMAKE_MATCH_1})
string(REGEX MATCH "HEATSHRINK_VERSION_MINOR[ \t]+([0-9]+)" _minor_match "${VERSION_CONTENT}")
set(HEATSHRINK_VERSION_MINOR ${CMAKE_MATCH_1})
string(REGEX MATCH "HEATSHRINK_VERSION_PATCH[ \t]+([0-9]+)" _patch_match "${VERSION_CONTENT}")
set(HEATSHRINK_VERSION_PATCH ${CMAKE_MATCH_1})
set(HEATSHRINK_VERSION ${HEATSHRINK_VERSION_MAJOR}.${HEATSHRINK_VERSION_MINOR}.${HEATSHRINK_VERSION_PATCH})

project(heatshrink C)

# sources are actually one directory up
set(srcdir ${PROJECT_SOURCE_DIR}/../)

# When making a DLL with MSVC, symbols are not exported by default.
# This can be changed with CMAKE_WINDOWS_EXPORT_ALL_SYMBOLS but
# it would really be better if an export header would be used instead.
set(CMAKE_WINDOWS_EXPORT_ALL_SYMBOLS ON)

# Add separate libraries for lib instances with static and dynamic memory allocation 
add_library(${PROJECT_NAME}_static ${srcdir}/heatshrink_decoder.c ${srcdir}/heatshrink_encoder.c)
add_library(${PROJECT_NAME}_dynamic ${srcdir}/heatshrink_decoder.c ${srcdir}/heatshrink_encoder.c)

add_library(heatshrink_noalloc ALIAS heatshrink_static)
add_library(heatshrink_dynalloc ALIAS heatshrink_dynamic)

# Add a default library with the same name as the project itself
add_library(${PROJECT_NAME} INTERFACE)
target_link_libraries(${PROJECT_NAME} INTERFACE ${PROJECT_NAME}_static)

# Set up downstream include paths.
target_include_directories(${PROJECT_NAME}_static PUBLIC $<INSTALL_INTERFACE:include>)
target_include_directories(${PROJECT_NAME}_dynamic PUBLIC $<INSTALL_INTERFACE:include>)

# Set up compile definitions to produce the static and dynamic memory allocating versions
target_compile_definitions(${PROJECT_NAME}_static PUBLIC HEATSHRINK_DYNAMIC_ALLOC=0)
target_compile_definitions(${PROJECT_NAME}_dynamic PUBLIC HEATSHRINK_DYNAMIC_ALLOC=1)

if (UNIX) # The heatshrink executable compiles only on Unix
  add_executable(${PROJECT_NAME}_cmd ${srcdir}/heatshrink.c)
  target_link_libraries(${PROJECT_NAME}_cmd ${PROJECT_NAME}_dynamic)
  set_target_properties(${PROJECT_NAME}_cmd PROPERTIES OUTPUT_NAME ${PROJECT_NAME})
endif ()

# Set up library versions. Used when shared libraries are built so that the right symlinks are created
foreach(memtype static dynamic)
  set_property(TARGET ${PROJECT_NAME}_${memtype} PROPERTY VERSION ${HEATSHRINK_VERSION})
  set_property(TARGET ${PROJECT_NAME}_${memtype} PROPERTY SOVERSION ${HEATSHRINK_VERSION_MAJOR})
endforeach()

# Testing

include(CTest)

# MSVC does not support variable length arrays, and the tests don't compile
if (NOT MSVC AND BUILD_TESTING)
  add_executable(test_heatshrink_static ${srcdir}/test_heatshrink_static.c)
  target_link_libraries(test_heatshrink_static heatshrink)

  add_executable(test_heatshrink_dynamic ${srcdir}/test_heatshrink_dynamic.c)
  target_link_libraries(test_heatshrink_dynamic heatshrink_dynamic)

  add_test(test_heatshrink_static test_heatshrink_static)
  add_test(test_heatshrink_dynamic test_heatshrink_dynamic)
endif ()

# Installation and export:

include(CMakePackageConfigHelpers)

# Installs a CMake specific package version file
write_basic_package_version_file(
    "${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}ConfigVersion.cmake"
    VERSION ${HEATSHRINK_VERSION}
    COMPATIBILITY AnyNewerVersion
)

# Exported targets differ on various platforms. The heatshrink executable
# is only installed on Unix platforms
set(_exported_targets ${PROJECT_NAME} ${PROJECT_NAME}_static ${PROJECT_NAME}_dynamic)
if (UNIX)
  list(APPEND _exported_targets ${PROJECT_NAME}_cmd)
endif ()

# Create an export set from the installable targets
install(TARGETS ${_exported_targets}
        EXPORT ${PROJECT_NAME}Targets
)

# Export the export set to a file named heatshrinkTargets.cmake into the 
# current build folder for now. This will be installed to the final destination
# later
export(EXPORT ${PROJECT_NAME}Targets 
       FILE "${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}Targets.cmake" 
       NAMESPACE ${PROJECT_NAME}::
)

include(GNUInstallDirs)
set(ConfigPackageLocation ${CMAKE_INSTALL_LIBDIR}/cmake/${PROJECT_NAME})

# Create the main config file used by CMake to find packages
configure_package_config_file(${CMAKE_CURRENT_SOURCE_DIR}/Config.cmake.in
  "${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}Config.cmake"
  INSTALL_DESTINATION ${ConfigPackageLocation}
)

# Install the actual public headers under <install_prefix>/include/heatshrink
install(
    FILES
      ${srcdir}/heatshrink_common.h
      ${srcdir}/heatshrink_config.h
      ${srcdir}/heatshrink_encoder.h
      ${srcdir}/heatshrink_decoder.h
    DESTINATION
      include/${PROJECT_NAME}
  )

# Install the previously created heatshrinkTargets.cmake describing the CMake targets
install(EXPORT ${PROJECT_NAME}Targets
  FILE
    ${PROJECT_NAME}Targets.cmake
  NAMESPACE
    ${PROJECT_NAME}::
  DESTINATION
    ${ConfigPackageLocation}
)

# Install the final CMake config file and version file for the package
# used directly by downstream find_package() calls
install(
  FILES
    "${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}Config.cmake"
    "${CMAKE_CURRENT_BINARY_DIR}/${PROJECT_NAME}ConfigVersion.cmake"
  DESTINATION
    ${ConfigPackageLocation}
)
