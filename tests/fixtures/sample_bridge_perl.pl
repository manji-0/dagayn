#!/usr/bin/perl
# Fixture for bridge detection tests (Perl).

sub run_command {
    system("git status");
}

sub read_config {
    open(my $fh, '<', "config.yaml") or die;
    return $fh;
}

sub write_output {
    open(my $fh, '>', "output.json") or die;
    print $fh "{}";
}

sub run_dynamic {
    my ($cmd) = @_;
    # Dynamic — LOW confidence
    system($cmd);
}
