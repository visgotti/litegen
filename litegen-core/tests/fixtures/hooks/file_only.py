# A hook file that defines only file_access: it closes every file fetch that
# carries x-test-file: deny.
def file_access(request, file, caller):
    if (request.get("headers") or {}).get("x-test-file") == "deny":
        return False
    return None
