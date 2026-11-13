"""Each of your customers opens only the files made for them.

litegen records who a generation was for (`x-litegen-end-user`, set here by
custom_auth.py) and passes it to `file_access` as file["created_by"]["end_user"];
whoever is asking now is caller["end_user"]. The hook runs on litegen's file
route, after litegen's own owner check, so it can only refuse. A file made for
no one in particular (no end user recorded) is left to that owner check.

    storage_access:
      mode: private
    hooks:
      - path: hooks/custom_auth.py
      - path: hooks/file_access_end_user.py
"""


def file_access(request, file, caller):
    made_for = file["created_by"].get("end_user")
    if made_for is not None and made_for != caller.get("end_user"):
        return False  # 403
    return None
