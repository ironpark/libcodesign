/* Links against the built archive and calls both entry points, so a release
 * artifact is only published if it actually links and runs on its target. */
#include <stdio.h>
#include <string.h>
#include "zapp_rcodesign.h"

int main(void) {
    /* A request with no certificate must fail without touching stdin. */
    char *error = zapp_rcodesign_run("{\"operation\":\"sign\",\"path\":\"x\",\"options\":{}}");
    if (error == NULL || strlen(error) == 0) {
        fprintf(stderr, "expected an error for a request without a certificate\n");
        return 1;
    }
    printf("error as expected: %s\n", error);
    zapp_rcodesign_free(error);
    zapp_rcodesign_free(NULL);

    /* Malformed JSON must be reported, not crash the caller. */
    error = zapp_rcodesign_run("not json");
    if (error == NULL) {
        fprintf(stderr, "expected an error for a malformed request\n");
        return 1;
    }
    zapp_rcodesign_free(error);
    return 0;
}
