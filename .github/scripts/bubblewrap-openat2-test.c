#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include "utils.h"

static int syscall_error = ENOSYS;
static struct stat inside;

long
probe_syscall (long number, ...)
{
  assert (number == __NR_openat2);
  errno = syscall_error;
  return -1;
}

char *
xstrdup (const char *text)
{
  char *copy = strdup (text);
  assert (copy);
  return copy;
}

static int
resolve (const char *path, int flags)
{
  int root = open ("/newroot", O_PATH | O_DIRECTORY | O_CLOEXEC);
  assert (root >= 0);
  int found = safe_openat (root, "/newroot", path, flags | O_CLOEXEC, 0);
  int saved = errno;
  close (root);
  errno = saved;
  return found;
}

static void
assert_inside (int fd)
{
  struct stat found;
  assert (fd >= 0 && fstat (fd, &found) == 0);
  assert (found.st_dev == inside.st_dev && found.st_ino == inside.st_ino);
  close (fd);
}

static void
run_case (int which)
{
  switch (which)
    {
    case 0: assert_inside (resolve ("/file", O_PATH)); break;
    case 1: assert_inside (resolve ("/absolute", O_PATH)); break;
    case 2: assert_inside (resolve ("/relative", O_PATH)); break;
    case 3: assert_inside (resolve ("/dir/../../file", O_PATH)); break;
    case 4:
      assert (resolve ("/escape", O_PATH) == -1 && errno == ENOENT); break;
    case 5:
      assert (resolve ("/loop", O_PATH) == -1 && errno == ELOOP); break;
    case 6:
      { int fd = resolve ("/absolute", O_PATH | O_NOFOLLOW); struct stat found;
        assert (fd >= 0 && fstat (fd, &found) == 0 && S_ISLNK (found.st_mode)); close (fd); }
      break;
    case 7:
      { int fd = resolve ("/dir", O_DIRECTORY); assert (fd >= 0); close (fd); } break;
    case 8:
      assert (resolve ("/file", O_DIRECTORY) == -1 && errno == ENOTDIR); break;
    case 9:
      assert (resolve ("/file", O_RDONLY) == -1 && errno == EOPNOTSUPP); break;
    case 10:
      assert (resolve ("/file", O_PATH | O_TRUNC) == -1 && errno == EOPNOTSUPP); break;
    case 11:
      assert (resolve ("/new", O_PATH | O_CREAT) == -1 && errno == EOPNOTSUPP);
      assert (access ("/newroot/new", F_OK) == -1 && errno == ENOENT); break;
    case 12:
      syscall_error = EPERM;
      assert (resolve ("/file", O_PATH) == -1 && errno == EPERM); break;
    case 13:
      syscall_error = EINVAL;
      assert (resolve ("/file", O_PATH) == -1 && errno == EINVAL); break;
    case 14:
      { int fd = open ("/newroot", O_PATH | O_DIRECTORY);
        assert (fd >= 0);
        assert (safe_openat (fd, "/tmp", "/file", O_PATH, 0) == -1 && errno == EOPNOTSUPP);
        close (fd); }
      break;
    case 15:
      { int fd = resolve ("/", O_DIRECTORY); struct stat found, root;
        assert (fd >= 0 && fstat (fd, &found) == 0 && stat ("/newroot", &root) == 0);
        assert (found.st_dev == root.st_dev && found.st_ino == root.st_ino); close (fd); }
      break;
    case 16:
      {
        pid_t racer = fork ();
        assert (racer >= 0);
        if (racer == 0)
          {
            for (unsigned i = 0;; i++)
              {
                unlink ("/newroot/next");
                assert (symlink (i % 2 ? "file" : "../tmp/outside", "/newroot/next") == 0);
                assert (rename ("/newroot/next", "/newroot/race") == 0);
              }
          }
        for (unsigned i = 0; i < 10000; i++)
          {
            int fd = resolve ("/race", O_PATH);
            if (fd >= 0) assert_inside (fd);
            else assert (errno == ENOENT || errno == ENOTDIR || errno == ELOOP);
          }
        assert (kill (racer, SIGTERM) == 0);
        assert (waitpid (racer, NULL, 0) == racer);
      }
      break;
    default: abort ();
    }
}

int
main (void)
{
  const char *names[] = {
    "ordinary descriptor", "absolute symlink stays rooted", "relative symlink",
    "dot-dot cannot leave root", "external absolute symlink refused", "symlink loop refused",
    "O_NOFOLLOW retained", "directory descriptor", "directory requirement retained",
    "ordinary file opens refused", "truncate refused", "create refused without side effect",
    "EPERM never falls back", "EINVAL never falls back", "unexpected root refused",
    "root directory descriptor", "10000 concurrent symlink replacements stay contained"
  };
  int file = open ("/newroot/file", O_CREAT | O_EXCL | O_RDWR, 0600);
  assert (file >= 0 && write (file, "inside", 6) == 6 && fstat (file, &inside) == 0);
  close (file);
  file = open ("/tmp/outside", O_CREAT | O_EXCL | O_RDWR, 0600);
  assert (file >= 0 && write (file, "outside", 7) == 7); close (file);
  assert (mkdir ("/newroot/dir", 0700) == 0);
  assert (symlink ("/file", "/newroot/absolute") == 0);
  assert (symlink ("file", "/newroot/relative") == 0);
  assert (symlink ("/tmp/outside", "/newroot/escape") == 0);
  assert (symlink ("loop", "/newroot/loop") == 0);
  for (int i = 0; i < (int) (sizeof (names) / sizeof (names[0])); i++)
    {
      pid_t child = fork ();
      assert (child >= 0);
      if (child == 0) { alarm (20); run_case (i); _exit (0); }
      int status;
      assert (waitpid (child, &status, 0) == child);
      if (!WIFEXITED (status) || WEXITSTATUS (status) != 0)
        { fprintf (stderr, "FAIL %s\n", names[i]); return 1; }
      printf ("PASS %s\n", names[i]);
    }
  return 0;
}
