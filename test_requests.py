import requests
import traceback

url = "https://api.modrinth.com/v2/project/iris"
headers = {"User-Agent": "TrailerPackTest/1.0"}
print(f"requests version: {requests.__version__}")
print(f"requests file:    {requests.__file__}")

try:
    r = requests.get(url, headers=headers, timeout=30)
    print(f"Response type:    {type(r)}")
    print(f"Response module:  {type(r).__module__}")
    print(f"Has status_code:  {hasattr(r, 'status_code')}")
    print(f"status_code:      {getattr(r, 'status_code', 'MISSING')}")
    print(f"Status:           {getattr(r, 'status', 'MISSING')}")
    print(f"Has status:       {hasattr(r, 'status')}")
    print(f"URL:              {r.url}")
    print(f"OK:               {r.ok}")
    data = r.json()
    print(f"JSON keys:        {list(data.keys())[:5]}")
except Exception:
    print("EXCEPTION:")
    traceback.print_exc()