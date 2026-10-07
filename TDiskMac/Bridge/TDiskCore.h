#ifndef TDISK_CORE_H
#define TDISK_CORE_H
#include <stdint.h>
#include <stddef.h>
// Handles are confined to the Swift backend actor. Strings are owned by Rust
// and must be released with tdisk_string_free, not free().
void *tdisk_scan_create(const char *path);
void tdisk_scan_cancel(void *handle);
void tdisk_scan_free(void *handle);
char *tdisk_scan_snapshot(void *handle, const char *folder_id, uint64_t previous_revision);
char *tdisk_scan_issues(void *handle);
char *tdisk_trash_prepare(void *handle, const char *item_id);
char *tdisk_trash_execute(void *handle);
void tdisk_string_free(char *value);
#endif
