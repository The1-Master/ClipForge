# Minimal HTTP/1 WSGI profile. No request/header/body logging.
bind = "127.0.0.1:4173"
workers = 1
worker_class = "gthread"
threads = 8
timeout = 120
graceful_timeout = 30
keepalive = 2
max_requests = 2000
max_requests_jitter = 100
limit_request_line = 1024
limit_request_fields = 32
limit_request_field_size = 4096
worker_tmp_dir = "/var/lib/clipforge-ingest"
accesslog = None
errorlog = "-"
loglevel = "warning"
capture_output = False
forwarded_allow_ips = "127.0.0.1"
umask = 0o077

