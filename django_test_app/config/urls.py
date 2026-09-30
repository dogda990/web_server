from django.urls import path

from . import views

urlpatterns = [
    path("get", views.get_endpoint),
    path("post", views.post_endpoint),
    path("put", views.put_endpoint),
    path("patch", views.patch_endpoint),
    path("delete", views.delete_endpoint),
    path("head", views.head_endpoint),
    path("options", views.options_endpoint),
    path("methods", views.methods_echo),
    path("rps_plain", views.rps_plain),
    path("rps_heavy", views.rps_heavy),
]
