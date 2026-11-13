// A hook file that defines only file_access: it closes every file fetch that
// carries x-test-file: deny.
export function file_access(request, file, caller) {
  if ((request.headers ?? {})["x-test-file"] === "deny") return false;
  return undefined;
}
