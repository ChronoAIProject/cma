/* Exercise the real pinned network.c against deterministic rtnetlink replies.
 * No namespaces, network interfaces, or actual sockets are created here. */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <net/if.h>
#include <netinet/in.h>
#include <linux/loop.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <setjmp.h>
#include "utils.h"

static int probe_socket (int, int, int);
static int probe_bind (int, const struct sockaddr *, socklen_t);
static ssize_t probe_sendto (int, const void *, size_t, int,
                             const struct sockaddr *, socklen_t);
static ssize_t probe_recv (int, void *, size_t, int);
static unsigned int probe_if_nametoindex (const char *);
static int netlink_fd;

#define socket probe_socket
#define bind probe_bind
#define sendto probe_sendto
#define recv probe_recv
#define if_nametoindex probe_if_nametoindex
#include "network.c"
#undef socket
#undef bind
#undef sendto
#undef recv
#undef if_nametoindex

static jmp_buf failed;
static bool existing_address;
static int address_error, link_error, ack_error;
static bool bad_sequence, bad_pid, send_failure, receive_failure;
static uint32_t sequence;
static unsigned int address_requests, link_requests;

void
die_with_error (const char *format, ...)
{
  (void) format;
  longjmp (failed, 1);
}

static unsigned int
probe_if_nametoindex (const char *name)
{
  assert (strcmp (name, "lo") == 0);
  return 1;
}

static int
probe_socket (int domain, int type, int protocol)
{
  assert (domain == AF_NETLINK);
  assert (type == (SOCK_RAW | SOCK_CLOEXEC));
  assert (protocol == NETLINK_ROUTE);
  netlink_fd = open ("/dev/null", O_RDONLY | O_CLOEXEC);
  assert (netlink_fd >= 0);
  return netlink_fd;
}

static int
probe_bind (int fd, const struct sockaddr *address, socklen_t length)
{
  const struct sockaddr_nl *nl = (const struct sockaddr_nl *) address;
  assert (fd == netlink_fd && length == sizeof (*nl));
  assert (nl->nl_family == AF_NETLINK && nl->nl_pid == (uint32_t) getpid ());
  assert (nl->nl_groups == 0);
  return 0;
}

static ssize_t
probe_sendto (int fd, const void *buffer, size_t length, int flags,
              const struct sockaddr *address, socklen_t address_length)
{
  const struct nlmsghdr *header = buffer;
  const struct sockaddr_nl *target = (const struct sockaddr_nl *) address;
  assert (fd == netlink_fd && flags == 0 && length == header->nlmsg_len);
  assert (address_length == sizeof (*target));
  assert (target->nl_family == AF_NETLINK && target->nl_pid == 0);
  assert (header->nlmsg_pid == (uint32_t) getpid ());
  assert ((header->nlmsg_flags & (NLM_F_REQUEST | NLM_F_ACK)) ==
          (NLM_F_REQUEST | NLM_F_ACK));
  sequence = header->nlmsg_seq;
  if (send_failure)
    { errno = EPERM; return -1; }
  if (header->nlmsg_type == RTM_NEWADDR)
    {
      const struct ifaddrmsg *msg = NLMSG_DATA (header);
      const struct rtattr *attribute = IFA_RTA (msg);
      int remaining = IFA_PAYLOAD (header);
      bool local_seen = false, address_seen = false;
      assert (++address_requests == 1 && link_requests == 0);
      assert (msg->ifa_family == AF_INET && msg->ifa_prefixlen == 8);
      assert (msg->ifa_flags == IFA_F_PERMANENT && msg->ifa_scope == RT_SCOPE_HOST);
      assert (msg->ifa_index == 1);
      assert ((header->nlmsg_flags & NLM_F_CREATE) != 0);
      assert ((header->nlmsg_flags & ~(NLM_F_REQUEST | NLM_F_ACK |
                                      NLM_F_CREATE | NLM_F_EXCL)) == 0);
      for (; RTA_OK (attribute, remaining); attribute = RTA_NEXT (attribute, remaining))
        {
          assert (RTA_PAYLOAD (attribute) == sizeof (struct in_addr));
          assert (((struct in_addr *) RTA_DATA (attribute))->s_addr == htonl (INADDR_LOOPBACK));
          if (attribute->rta_type == IFA_LOCAL)
            { assert (!local_seen); local_seen = true; }
          else
            { assert (attribute->rta_type == IFA_ADDRESS && !address_seen); address_seen = true; }
        }
      assert (remaining == 0 && local_seen && address_seen);
      ack_error = address_error;
      if (!ack_error && existing_address && (header->nlmsg_flags & NLM_F_EXCL))
        ack_error = EEXIST;
    }
  else
    {
      const struct ifinfomsg *msg = NLMSG_DATA (header);
      assert (header->nlmsg_type == RTM_NEWLINK);
      assert (++link_requests == 1 && address_requests == 1);
      assert (header->nlmsg_flags == (NLM_F_REQUEST | NLM_F_ACK));
      assert (msg->ifi_index == 1 && msg->ifi_family == AF_UNSPEC);
      assert (msg->ifi_flags == IFF_UP && msg->ifi_change == IFF_UP);
      ack_error = link_error;
    }
  return (ssize_t) length;
}

static ssize_t
probe_recv (int fd, void *buffer, size_t length, int flags)
{
  struct nlmsghdr *header = buffer;
  struct nlmsgerr *error;
  assert (fd == netlink_fd && flags == 0 && length >= NLMSG_SPACE (sizeof (*error)));
  if (receive_failure)
    { errno = EIO; return -1; }
  memset (buffer, 0, length);
  header->nlmsg_len = NLMSG_LENGTH (sizeof (*error));
  header->nlmsg_type = NLMSG_ERROR;
  header->nlmsg_seq = sequence + (bad_sequence ? 1 : 0);
  header->nlmsg_pid = getpid () + (bad_pid ? 1 : 0);
  error = NLMSG_DATA (header);
  error->error = -ack_error;
  return header->nlmsg_len;
}

static void
run_case (const char *name, bool expect_success, unsigned int expected_links)
{
  volatile bool success = false;
  address_requests = link_requests = 0;
  if (setjmp (failed) == 0)
    { loopback_setup (); success = true; }
  if (!success)
    close (netlink_fd);
  if (success != expect_success || link_requests != expected_links)
    {
      fprintf (stderr, "FAIL %s: success=%d, link requests=%u\n",
               name, success, link_requests);
      exit (1);
    }
  printf ("PASS %s\n", name);
}

int
main (void)
{
  run_case ("fresh loopback reaches link setup", true, 1);
  existing_address = true;
  run_case ("preconfigured loopback reaches link setup", true, 1);
  existing_address = false;
  address_error = EPERM;
  run_case ("address permission failure remains fatal", false, 0);
  address_error = EEXIST;
  run_case ("unexpected address conflict remains fatal", false, 0);
  address_error = 0;
  link_error = EPERM;
  run_case ("link setup failure remains fatal", false, 1);
  link_error = 0;
  bad_sequence = true;
  run_case ("mismatched reply sequence remains fatal", false, 0);
  bad_sequence = false;
  bad_pid = true;
  run_case ("mismatched reply process remains fatal", false, 0);
  bad_pid = false;
  send_failure = true;
  run_case ("request send failure remains fatal", false, 0);
  send_failure = false;
  receive_failure = true;
  run_case ("reply receive failure remains fatal", false, 0);
  return 0;
}
