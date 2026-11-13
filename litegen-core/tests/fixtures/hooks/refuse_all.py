# Test hook that refuses every request it sees, so a test can tell whether a
# route reaches the hooks at all.
class Forbidden(Exception):
    status_code = 403
    detail = "refused by refuse_all"


def pre_call(request):
    raise Forbidden("refused by refuse_all")
